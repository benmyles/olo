//! A window's stage: what it last showed, kept on the display thread,
//! and the frames drawn from it.
//!
//! A window's looper lays out and paints what changed, then commits:
//! every view's drawing ([`Recording`], kept from commit to commit unless
//! the view drew again), where each goes ([`Item`]) and what scrolls
//! ([`Layer`]). The stage finds what differs from the commit before (its
//! damage), puts a frame together from the recordings and has the
//! renderer draw it. It needs nothing more from the looper to draw, so a
//! frame that only scrolls or animates is drawn while the looper works.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use collections::FxHashMap;
use olo_app::{Latest, LooperHandle};
use smallvec::SmallVec;

use crate::{
    Bounds, Elastic, FrameRegion, FrameStats, GpuSpecs, ImeInput, Pixels, PlatformAtlas, PlatformInput, PlatformWindow, Point, RequestFrameOptions, ScaledPixels, Scene, ScrollWheelEvent, Size,
    TextRendering, TouchPhase, WindowId, point, px, size,
};

/// What a view drew, kept: its display list. Made on the window's
/// looper, read by the display thread; never changed once made (a view
/// that draws again makes another).
pub(crate) struct Recording {
    pub(crate) scene: Scene,
    /// What it painted reaches, where it was painted.
    pub(crate) bounds: Bounds<ScaledPixels>,
}

/// A recording's place in a frame.
#[derive(Clone)]
pub(crate) struct Item {
    pub(crate) list: Arc<Recording>,
    /// How far it is from where it was painted (whole device pixels),
    /// before its layer's scrolling.
    pub(crate) by: Point<ScaledPixels>,
    /// What holds it clips it to this, in its layer's own coordinates.
    pub(crate) clip: Bounds<ScaledPixels>,
    /// Which layer it scrolls with (an index into the commit's layers).
    pub(crate) layer: u32,
}

impl Item {
    fn same_place(&self, other: &Item) -> bool {
        Arc::ptr_eq(&self.list, &other.list) && self.by == other.by && self.clip == other.clip && self.layer == other.layer
    }

    /// What shows of it, in its layer's coordinates.
    fn shown(&self) -> Bounds<ScaledPixels> {
        Bounds { origin: self.list.bounds.origin + self.by, size: self.list.bounds.size }.intersect(&self.clip)
    }
}

/// Something that scrolls: what's in it moves together, by an offset the
/// display thread keeps (so it can move it without the looper: the
/// wheel and the touchpad scroll it here, and the looper hears where it
/// got to).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    /// The same from commit to commit for the same scroller (0: the
    /// window itself, which doesn't scroll).
    pub(crate) id: u64,
    /// The layer it's in.
    pub(crate) parent: u32,
    /// Where it shows, in its parent's coordinates (within everything
    /// that clips it there).
    pub(crate) viewport: Bounds<ScaledPixels>,
    /// How far it scrolls each way (nothing, a way it doesn't), and how
    /// big its view is, in points.
    pub(crate) max: Size<Pixels>,
    pub(crate) view: Size<Pixels>,
    /// Which ways it scrolls: across, down.
    pub(crate) axes: (bool, bool),
    /// Where the looper put it, if it moved it (a view scrolled to, a
    /// scroll bar dragged) and which of its moves this is: otherwise
    /// the display thread's own stands.
    pub(crate) set: Option<(Point<Pixels>, u32)>,
}

/// Where the wheel means something, in painting order (the last one
/// under the pointer has it).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WheelRegion {
    /// The layer it's in, and where in it.
    pub(crate) layer: u32,
    pub(crate) rect: Bounds<ScaledPixels>,
    pub(crate) kind: WheelKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum WheelKind {
    /// A scroller's view: the wheel scrolls that layer.
    Scroller(u32),
    /// Something that takes the wheel itself (its looper hears it), or
    /// that nothing under it is to get the wheel through.
    Taker,
}

/// Where a scroller is: (its layer's id, where it's scrolled to, where
/// its content shows, which of the looper's own moves it has taken).
pub(crate) type ScrollReport = (u64, Point<Pixels>, Point<Pixels>, u32);

/// What a window shows now, from its looper.
pub(crate) struct Commit {
    /// The size and scale it was laid out for.
    pub(crate) size: Size<Pixels>,
    pub(crate) scale: f32,
    /// Every recording that shows, in painting order.
    pub(crate) items: Vec<Item>,
    pub(crate) layers: Vec<Layer>,
    pub(crate) wheel: Vec<WheelRegion>,
    /// How long the looper worked on it, from taking what led to it
    /// (input): the frame's `ui` time.
    pub(crate) work: Duration,
    /// Everything is to be drawn again whatever matches (the look or the
    /// text's rendering changed what the same drawings mean).
    pub(crate) everything: bool,
    /// The looper animates: it wants [`WindowEvent::Frame`] as the next
    /// frame starts.
    pub(crate) animating: bool,
    /// Where other programs' pictures show (`Painter::place_external`).
    pub(crate) externals: Vec<(u64, Bounds<Pixels>)>,
    /// Where what scrolls itself was as it drew
    /// (`Painter::note_scroll`), for the frames' marks.
    pub(crate) noted: Vec<[f32; 4]>,
    /// The last input the looper had taken when it made this
    /// (`WindowEvent::Input`'s number): a frame kept back for the answer
    /// to input (`INPUT_HOLD`) is kept until the commit that has it, not
    /// drawn with one made before the looper got to it (a worker's
    /// layout or colours arriving in between).
    pub(crate) answered: u64,
}

/// What the display thread tells a window's looper.
#[derive(Debug)]
pub(crate) enum WindowEvent {
    /// Input, its number among what this window's stage has sent its
    /// looper (0: not from the stage): what the looper commits says how
    /// far through them it has got (`Commit::answered`); and whether the
    /// right Ctrl (app_server's ⌘) was held as it was made, which by the
    /// time the looper takes it may no longer be so
    /// (`right_control_held`).
    Input(PlatformInput, u64, bool),
    Ime(ImeInput),
    /// The window is this size now, at this scale: laid out again and
    /// committed for it.
    Resized {
        size: Size<Pixels>,
        scale: f32,
    },
    /// It has the keyboard, or lost it.
    Active(bool),
    /// The pointer came over it, or left.
    Hovered(bool),
    Moved,
    /// The person asked to close it (its tab's close box).
    CloseRequested,
    /// Its surface is gone.
    Closed,
    /// How text is drawn changed (it moved to another display).
    TextRendering(TextRendering),
    /// A frame is starting: what animates takes its next step.
    Frame,
    /// A frame can be drawn now: what the looper kept for it
    /// ([`FramePace`]) is laid out, painted and committed.
    FrameDue,
    /// The display it's on, in points.
    Display(Option<Bounds<Pixels>>),
    /// Where what scrolls is now (the wheel or the touchpad moved it,
    /// or an end let go of it).
    Scrolled(Vec<ScrollReport>),
    /// How app_server holds the window changed.
    State(WindowState),
    /// The compositor gave it its first size (told by `Resized` just
    /// before if it isn't the one it opened at): a window that waited
    /// for it ([`Opened::awaits_size`]) is laid out and drawn now.
    Sized,
}

/// How app_server holds a window: its size isn't the program's to set
/// while any of these is so.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowState {
    pub maximized: bool,
    pub fullscreen: bool,
    /// The sides of it that are against a tile's.
    pub tiling: crate::Tiling,
}

impl WindowState {
    /// Whether app_server holds its size (a side of it in a tile,
    /// maximized, full screen).
    pub fn held(&self) -> bool {
        let t = self.tiling;
        self.maximized || self.fullscreen || t.top || t.bottom || t.left || t.right
    }
}

/// How the display thread reaches a window's looper.
#[derive(Clone)]
pub(crate) struct WindowLink {
    pub(crate) looper: LooperHandle,
    pub(crate) id: WindowId,
}

impl WindowLink {
    pub(crate) fn send(&self, event: WindowEvent) {
        let id = self.id;
        self.looper.post(move |looper| crate::window::deliver(looper, id, event));
    }
}

/// What a looper gets back for opening a window.
pub(crate) struct Opened {
    pub(crate) pace: Arc<FramePace>,
    pub(crate) size: Size<Pixels>,
    pub(crate) scale: f32,
    pub(crate) atlas: Arc<dyn PlatformAtlas>,
    pub(crate) text_rendering: TextRendering,
    pub(crate) surface_id: Option<u32>,
    pub(crate) gpu: Option<GpuSpecs>,
    pub(crate) display: Option<Bounds<Pixels>>,
    /// Its `size` stands in for the compositor's, not yet given
    /// (`PlatformWindow::awaits_size`): nothing is laid out or drawn
    /// until the stage says it came (`WindowEvent::Sized`).
    pub(crate) awaits_size: bool,
}

/// How a window's looper and its stage agree on drawing something once
/// a frame, at the last moment ([`crate::Cx::invalidate_at_frame`]): a
/// view fed faster than frames are shown (a terminal's shell printing)
/// isn't laid out and painted for frames nobody sees.
///
/// The stage draws a commit at once when it's free, and then waits for
/// the compositor's word that the frame was shown before it draws
/// another. While it waits, the looper keeps what was invalidated for
/// the frame, and says so here; as the compositor's word comes and the
/// stage has nothing of its own to draw, it asks for it
/// ([`WindowEvent::FrameDue`]), and draws the commit that answers at
/// once: what shows is as it was when the frame was asked for, as GPUI
/// drew everything. (Here only what asks to be is kept for it: the rest
/// is committed as it changes, and the stage draws the latest.) What's
/// kept also goes with anything else the looper lays out meanwhile, and
/// with each step of something animating, so it is never behind them.
#[derive(Default)]
pub(crate) struct FramePace {
    /// A commit is on its way to the stage, or drawn and not yet shown:
    /// the next frame is drawn when the compositor asks for it.
    pub(crate) waiting: AtomicBool,
    /// The looper keeps something for that frame.
    pub(crate) held: AtomicBool,
    /// Nothing asks this window for frames (it's drawn nowhere: a test's
    /// window with no display): nothing is kept for one.
    pub(crate) unpaced: bool,
    /// The stage keeps a frame back for the looper's answer to input
    /// (`INPUT_HOLD`): a looper that makes nothing to show of what it
    /// was sent says so, and the frame is drawn.
    pub(crate) holding: AtomicBool,
}

/// How long a new size waits for the looper to lay out for it before
/// the old layout is drawn at it (a looper that's stuck doesn't keep the
/// window from following the person's hand).
const SIZE_WAIT: Duration = Duration::from_millis(250);

/// A scroller as the display thread keeps it.
#[derive(Default)]
struct Scroller {
    /// Where it's scrolled to, within its ends: zero at its start, down
    /// to minus how far it goes (as GPUI's), in points.
    offset: Point<Pixels>,
    /// Its ends' stretch (fingers pull past them, a glide bounces).
    elastic: Elastic,
    /// Where its content shows, as last drawn: `offset`, and past an
    /// end as far as it's stretched.
    shown: Point<Pixels>,
    /// The last of the looper's own moves it took.
    epoch: u32,
}

/// Whose a turn of the wheel is.
#[derive(Clone, Copy, PartialEq)]
enum Wheel {
    /// Something here scrolls with it (if it isn't at its end).
    Here,
    /// Something takes it itself: its looper's.
    Taker,
    /// Nothing there does anything with it.
    Nobody,
}

struct Showing {
    commit: Option<Commit>,
    scrollers: FxHashMap<u64, Scroller>,
    /// Something changed since the last frame drawn.
    dirty: bool,
    /// How long the looper worked on what the next frame shows (its
    /// commits since the last frame drawn): the frame's `ui` time.
    work: Duration,
    /// What changed, in device pixels (empty with `dirty`: all of it).
    damage: Vec<Bounds<ScaledPixels>>,
    /// The size the window is now, and since when the commit hasn't
    /// matched it.
    size: Size<Pixels>,
    scale: f32,
    resized_at: Option<Instant>,
    stats: FrameStats,
    scene: Scene,
}

/// A part of the window is to be drawn again.
fn add_damage(dirty: &mut bool, damage: &mut Vec<Bounds<ScaledPixels>>, rect: Bounds<ScaledPixels>) {
    // (Dirty with no damage: all of it already is.)
    if !(*dirty && damage.is_empty()) && !rect.is_empty() {
        damage.push(rect);
        *dirty = true;
    }
}

pub(crate) struct Stage {
    id: WindowId,
    window: RefCell<Option<Box<dyn PlatformWindow>>>,
    link: WindowLink,
    showing: RefCell<Showing>,
    waker: RefCell<Option<Rc<dyn Fn()>>>,
    /// Pointer motion not yet taken by the looper: the latest stands for
    /// all of it.
    motion: RefCell<Latest<(PlatformInput, u64, bool)>>,
    /// Where the scrollers are, not yet taken by the looper.
    scrolled: Latest<Vec<ScrollReport>>,
    /// How app_server held the window, as the looper was last told.
    state: Cell<WindowState>,
    closed: Cell<bool>,
    /// Where other programs' pictures were last placed.
    externals: RefCell<Vec<(u64, Bounds<Pixels>)>>,
    pace: Arc<FramePace>,
    /// When the looper was last asked for what it kept for a frame
    /// (`OLO_FRAME_PHASES` says how long its answer took to draw).
    asked: Cell<Option<Instant>>,
    /// Frames drawn in a row while the looper kept something for one.
    passed_over: Cell<u8>,
    /// Input went to the looper and what it makes of it hasn't come
    /// back (a commit, or word that nothing came of it): how many have
    /// gone, how far the looper has said it's got, and when the first
    /// not yet answered went.
    sent: Cell<u64>,
    answered: Cell<u64>,
    input_at: Cell<Instant>,
    /// The looper waits for the window's first size (`Opened::awaits_size`):
    /// told with the first frame, which comes once it's given.
    awaits_size: Cell<bool>,
    /// When it was made, until its first frame is drawn
    /// (`OLO_FRAME_PHASES` says how long that took).
    made: Cell<Option<Instant>>,
}

/// How long a frame waits for the looper's answer to input that has
/// just gone to it. The compositor sends input and then calls for a
/// frame, at once: a wheel's step for a view that scrolls itself (the
/// editor, a terminal) goes to the looper, whose commit comes back half
/// a millisecond later. With nothing else to draw, the call draws
/// nothing and the commit is drawn as it comes, in time for the
/// display's next frame. But with something else to draw (colours or
/// layout a worker brought, a caret), the call drew that, and the
/// commit, a moment too late, waited for the next call: shown a frame
/// later, and so was every step after it, with one frame in which
/// nothing moved. So what the call would draw waits for the answer and
/// is drawn with it, once. No longer than this: a looper that takes its
/// time holds nothing up.
const INPUT_HOLD: Duration = Duration::from_millis(4);

impl Stage {
    pub(crate) fn new(id: WindowId, window: Box<dyn PlatformWindow>, link: WindowLink) -> (Rc<Stage>, Opened) {
        let size = window.content_size();
        let scale = window.scale_factor();
        let pace = Arc::new(FramePace { unpaced: window.frame_waker().is_none(), ..Default::default() });
        let awaits_size = window.awaits_size();
        let opened = Opened {
            pace: pace.clone(),
            size,
            scale,
            atlas: window.sprite_atlas(),
            text_rendering: window.text_rendering(),
            surface_id: window.surface_id(),
            gpu: window.gpu_specs(),
            display: window.display().map(|d| d.bounds()),
            awaits_size,
        };
        let waker = window.frame_waker();
        let stage = Rc::new(Stage {
            id,
            window: RefCell::new(None),
            link,
            showing: RefCell::new(Showing {
                commit: None,
                scrollers: FxHashMap::default(),
                dirty: false,
                work: Duration::ZERO,
                damage: Vec::new(),
                size,
                scale,
                resized_at: Some(Instant::now()),
                stats: FrameStats::default(),
                scene: Scene::default(),
            }),
            waker: RefCell::new(waker),
            motion: RefCell::new(Latest::new()),
            scrolled: Latest::new(),
            state: Cell::new(WindowState::default()),
            closed: Cell::new(false),
            externals: RefCell::default(),
            pace,
            asked: Cell::new(None),
            passed_over: Cell::new(0),
            sent: Cell::new(0),
            answered: Cell::new(0),
            input_at: Cell::new(Instant::now()),
            awaits_size: Cell::new(awaits_size),
            made: Cell::new(Some(Instant::now())),
        });
        let weak = Rc::downgrade(&stage);
        let on = |f: fn(&Stage)| {
            let weak = weak.clone();
            Box::new(move || {
                if let Some(stage) = weak.upgrade() {
                    f(&stage)
                }
            })
        };
        window.on_request_frame(Box::new({
            let weak = weak.clone();
            move |options| weak.upgrade().is_some_and(|stage| stage.frame(options))
        }));
        window.on_input(Box::new({
            let weak = weak.clone();
            move |input| {
                if let Some(stage) = weak.upgrade() {
                    stage.input(input)
                }
            }
        }));
        window.on_ime(Box::new({
            let weak = weak.clone();
            move |ime| {
                if let Some(stage) = weak.upgrade() {
                    stage.break_motion();
                    stage.link.send(WindowEvent::Ime(ime))
                }
            }
        }));
        window.on_active_status_change(Box::new({
            let weak = weak.clone();
            move |active| {
                if let Some(stage) = weak.upgrade() {
                    stage.break_motion();
                    stage.link.send(WindowEvent::Active(active))
                }
            }
        }));
        window.on_hover_status_change(Box::new({
            let weak = weak.clone();
            move |hovered| {
                if let Some(stage) = weak.upgrade() {
                    stage.break_motion();
                    stage.link.send(WindowEvent::Hovered(hovered))
                }
            }
        }));
        window.on_resize(Box::new({
            let weak = weak.clone();
            move |size, scale| {
                if let Some(stage) = weak.upgrade() {
                    stage.resized(size, scale)
                }
            }
        }));
        window.on_moved(on(|stage| {
            stage.tell_state();
            stage.link.send(WindowEvent::Moved);
            let display = stage.window.borrow().as_ref().map(|w| w.display().map(|d| d.bounds()));
            if let Some(display) = display {
                stage.link.send(WindowEvent::Display(display));
            }
        }));
        window.on_should_close(Box::new({
            let weak = weak.clone();
            move || {
                // The window's looper decides (unsaved changes): it closes
                // the window itself if it's to go.
                if let Some(stage) = weak.upgrade() {
                    stage.link.send(WindowEvent::CloseRequested);
                }
                false
            }
        }));
        window.on_close(Box::new({
            let weak = weak.clone();
            move || {
                if let Some(stage) = weak.upgrade() {
                    stage.closed.set(true);
                    stage.link.send(WindowEvent::Closed);
                }
            }
        }));
        window.on_appearance_changed(on(|stage| {
            stage.tell_state();
            let rendering = stage.window.borrow().as_ref().map(|w| w.text_rendering());
            if let Some(rendering) = rendering {
                stage.link.send(WindowEvent::TextRendering(rendering));
            }
        }));
        *stage.window.borrow_mut() = Some(window);
        (stage, opened)
    }

    pub(crate) fn id(&self) -> WindowId {
        self.id
    }

    /// Does something with the window (what its looper asked for: a
    /// title, a size, the pointer's shape).
    pub(crate) fn with_window<R>(&self, f: impl FnOnce(&mut dyn PlatformWindow) -> R) -> Option<R> {
        let mut window = self.window.borrow_mut();
        window.as_mut().map(|w| f(w.as_mut()))
    }

    /// Lets the window go: its surface is destroyed.
    pub(crate) fn close(&self) {
        self.closed.set(true);
        self.waker.borrow_mut().take();
        let window = self.window.borrow_mut().take();
        drop(window);
    }

    fn wake(&self) {
        let waker = self.waker.borrow().clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    /// Input goes to the looper: a frame that would be drawn before its
    /// answer waits for it ([`INPUT_HOLD`]).
    /// Its number goes with it.
    fn input_sent(&self) -> u64 {
        // (From the first of it not yet answered: more input to a looper
        // that's taking its time doesn't hold a frame any longer.)
        if !self.answering() {
            self.input_at.set(Instant::now());
        }
        let number = self.sent.get() + 1;
        self.sent.set(number);
        number
    }

    /// The looper hasn't said what it makes of the last input sent it.
    fn answering(&self) -> bool {
        self.answered.get() < self.sent.get()
    }

    /// Nothing to show came of the input the looper took (as far as
    /// number `upto`): what waited for its answer is drawn, if that was
    /// the last of it.
    pub(crate) fn input_answered(&self, upto: u64) {
        self.answered.set(self.answered.get().max(upto));
        if self.answering() {
            // (Not the last of it yet: the looper says again when it
            // has taken that.)
            self.pace.holding.store(true, Ordering::SeqCst);
        } else if self.showing.borrow().dirty {
            self.wake();
        }
    }

    /// The next motion starts afresh: something else was sent, and
    /// motion from before it mustn't be taken for motion after.
    fn break_motion(&self) {
        *self.motion.borrow_mut() = Latest::new();
    }

    fn input(&self, input: PlatformInput) {
        let id = self.id;
        // (This thread heard the keys: as they are now is as they were
        // when this was made.)
        let command = crate::platform::right_control_now();
        match input {
            PlatformInput::MouseMove(_) => {
                // (Motion gathers on its way: the last of it has the
                // number that answers for all it stood in for.)
                let number = self.input_sent();
                let motion = self.motion.borrow().clone();
                motion.put(&self.link.looper, (input, number, command), move |looper, (input, number, command)| crate::window::deliver(looper, id, WindowEvent::Input(input, number, command)));
            }
            // The wheel and the touchpad scroll here, at once: the
            // looper hears where things got to. What nothing here
            // scrolls is the looper's (something takes the wheel
            // itself).
            PlatformInput::ScrollWheel(event) => {
                let whose = self.scroll(&event);
                // (Nothing under the pointer scrolls or takes the wheel:
                // nobody is told. Its end is, though: what took the steps
                // before the pointer left it waits for that.)
                if whose == Wheel::Nobody && !matches!(event.touch_phase, TouchPhase::Ended) {
                    return;
                }
                if whose != Wheel::Here {
                    if whose == Wheel::Taker {
                        // (Its notches' ease isn't dropped for want of
                        // a frame drawn now: the looper's answer draws.)
                        if let Some(window) = self.window.borrow().as_ref() {
                            window.hold_wheel();
                        }
                    }
                    self.break_motion();
                    let number = self.input_sent();
                    self.link.send(WindowEvent::Input(PlatformInput::ScrollWheel(event), number, command));
                }
            }
            other => {
                self.break_motion();
                let number = self.input_sent();
                self.link.send(WindowEvent::Input(other, number, command));
            }
        }
    }

    /// Scrolls what's under the pointer, if something there scrolls;
    /// else says whose the wheel is.
    fn scroll(&self, event: &ScrollWheelEvent) -> Wheel {
        let mut showing = self.showing.borrow_mut();
        let showing = &mut *showing;
        let Some(commit) = &showing.commit else { return Wheel::Nobody };
        let scale = commit.scale;
        let places = layer_places(&commit.layers, &showing.scrollers, scale);
        let mut moved: Option<Bounds<ScaledPixels>> = None;
        let mut whose = Wheel::Nobody;
        if matches!(event.touch_phase, TouchPhase::Ended) {
            // The fingers lifted, or the glide stopped: every end they
            // held lets go. (The looper hears too: what takes the wheel
            // itself may be waiting for it.)
            for (i, layer) in commit.layers.iter().enumerate().skip(1) {
                let Some(scroller) = showing.scrollers.get_mut(&layer.id) else { continue };
                if scroller.elastic.scroll(&mut scroller.offset, Point::default(), event.kind, event.touch_phase) {
                    moved = Some(moved.map_or(places[i].1, |m| m.union(&places[i].1)));
                }
            }
        } else {
            let at = point(ScaledPixels(event.position.x.0 * scale), ScaledPixels(event.position.y.0 * scale));
            let delta = event.delta.pixel_delta(px(20.));
            for region in commit.wheel.iter().rev() {
                let Some((shift, clip)) = places.get(region.layer as usize).copied() else { continue };
                let rect = Bounds { origin: region.rect.origin + shift, size: region.rect.size }.intersect(&clip);
                if !rect.contains(&at) {
                    continue;
                }
                let WheelKind::Scroller(index) = region.kind else {
                    whose = Wheel::Taker;
                    break;
                };
                let Some(layer) = commit.layers.get(index as usize) else { continue };
                let Some(scroller) = showing.scrollers.get_mut(&layer.id) else { continue };
                // Which way: a wheel that only turns one way scrolls
                // what only scrolls the other (a row of tabs), and never
                // both at once.
                let (across, down) = layer.axes;
                let mut d = Point::default();
                if across {
                    d.x = if delta.x != px(0.) || down { delta.x } else { delta.y };
                }
                if down {
                    d.y = if delta.y != px(0.) || across { delta.y } else { delta.x };
                }
                if d.x != px(0.) && d.y != px(0.) {
                    if d.x.abs() > d.y.abs() {
                        d.y = px(0.);
                    } else {
                        d.x = px(0.);
                    }
                }
                // Nothing to scroll that way here: what holds it may
                // have.
                let room = (d.x != px(0.) && layer.max.width > px(0.)) || (d.y != px(0.) && layer.max.height > px(0.));
                if !room && !scroller.elastic.stretched() {
                    continue;
                }
                whose = Wheel::Here;
                if scroller.elastic.scroll(&mut scroller.offset, d, event.kind, event.touch_phase) {
                    moved = Some(places[index as usize].1);
                }
                break;
            }
        }
        if let Some(rect) = moved {
            add_damage(&mut showing.dirty, &mut showing.damage, rect);
            self.report(&showing.scrollers);
            self.wake();
        }
        whose
    }

    /// Tells the looper where the scrollers are.
    fn report(&self, scrollers: &FxHashMap<u64, Scroller>) {
        let report: Vec<ScrollReport> = scrollers.iter().map(|(id, s)| (*id, s.offset, s.shown_now(), s.epoch)).collect();
        let id = self.id;
        self.scrolled.put(&self.link.looper, report, move |looper, report| crate::window::deliver(looper, id, WindowEvent::Scrolled(report)));
    }

    /// Tells the looper how app_server holds the window, if that
    /// changed. (Before a new size: what's laid out for it knows.)
    fn tell_state(&self) {
        // (Not while something has the window: told with what comes
        // next.)
        let Ok(window) = self.window.try_borrow() else { return };
        let Some(window) = window.as_ref() else { return };
        let state = WindowState { maximized: window.is_maximized(), fullscreen: window.is_fullscreen(), tiling: window.tiling() };
        if self.state.replace(state) != state {
            self.link.send(WindowEvent::State(state));
        }
    }

    fn resized(&self, size: Size<Pixels>, scale: f32) {
        self.tell_state();
        {
            let mut showing = self.showing.borrow_mut();
            if showing.size == size && showing.scale == scale {
                return;
            }
            showing.size = size;
            showing.scale = scale;
            showing.resized_at.get_or_insert_with(Instant::now);
        }
        self.break_motion();
        self.link.send(WindowEvent::Resized { size, scale });
    }

    /// The looper's new commit: what it shows from now on.
    pub(crate) fn commit(&self, commit: Commit) {
        if self.closed.get() {
            return;
        }
        // Other programs' pictures: placed (or hidden) when where they
        // show changed, with the frame this commit draws.
        if *self.externals.borrow() != commit.externals {
            self.with_window(|window| window.place_externals(&commit.externals));
            *self.externals.borrow_mut() = commit.externals.clone();
        }
        self.answered.set(self.answered.get().max(commit.answered));
        {
            let mut showing = self.showing.borrow_mut();
            let showing = &mut *showing;
            // What scrolls: each layer's scroller is kept from commit to
            // commit (where it is is the display thread's), told its
            // ends, and put where the looper moved it if it did.
            let mut told = false;
            for layer in commit.layers.iter().skip(1) {
                let scroller = showing.scrollers.entry(layer.id).or_default();
                scroller.elastic.set_extent(layer.max, layer.view);
                let before = scroller.offset;
                if let Some((to, epoch)) = layer.set {
                    scroller.offset = to;
                    scroller.epoch = epoch;
                    told = true;
                }
                scroller.offset = point(scroller.offset.x.clamp(-layer.max.width, px(0.)), scroller.offset.y.clamp(-layer.max.height, px(0.)));
                told |= scroller.offset != before;
            }
            if showing.scrollers.len() + 1 > commit.layers.len() {
                showing.scrollers.retain(|id, _| commit.layers.iter().any(|l| l.id == *id));
            }
            if told {
                self.report(&showing.scrollers);
            }
            match (&showing.commit, commit.everything) {
                (Some(before), false) if before.scale == commit.scale && before.size == commit.size => {
                    let all = showing.dirty && showing.damage.is_empty();
                    if !all {
                        damage(before, &commit, &showing.scrollers, &mut showing.damage);
                        showing.dirty |= !showing.damage.is_empty();
                    }
                    // (A scroller the looper moved: the next frame finds
                    // where it shows now, and draws its view.)
                }
                _ => {
                    showing.dirty = true;
                    showing.damage.clear();
                }
            }
            if commit.size == showing.size && commit.scale == showing.scale {
                showing.resized_at = None;
            }
            showing.work += commit.work;
            showing.commit = Some(commit);
        }
        // (The looper's answer to what it was sent, if it had got to
        // the last of it: else its next is, and what's kept back for
        // that stays so.)
        if !self.answering() {
            self.pace.holding.store(false, Ordering::SeqCst);
        }
        self.wake();
    }

    /// Asks for a frame (something of the stage's own changed).
    pub(crate) fn redraw(&self) {
        {
            let mut showing = self.showing.borrow_mut();
            showing.dirty = true;
            showing.damage.clear();
        }
        self.wake();
    }

    /// Draws a frame if there's anything new to show. Whether it drew.
    fn frame(&self, options: RequestFrameOptions) -> bool {
        if self.closed.get() {
            return false;
        }
        // (The first frame comes with the first configure: its size, if
        // another, went to the looper as it came, `resized`.)
        if self.awaits_size.replace(false) {
            self.link.send(WindowEvent::Sized);
        }
        let asked = self.asked.take();
        let drew = self.draw_frame(options);
        // (What's drawn is shown when the compositor says so, and the
        // next frame drawn then: until it is, the looper keeps what's
        // invalidated for a frame.)
        self.pace.waiting.store(drew, Ordering::SeqCst);
        if drew {
            if let Some(asked) = asked
                && crate::stats::frame_phases()
            {
                eprintln!("frame due: drawn {:.3} ms after the looper was asked", asked.elapsed().as_secs_f64() * 1000.0);
            }
            // What the looper keeps waits for a frame of its own: asked
            // for now, its answer would wait a frame to be drawn, as it
            // was when asked, and the one after it too (while a shell
            // prints, every frame a frame late). Unless frames go by
            // with something else to draw in each (a scroller gliding):
            // then it goes in every other.
            if self.pace.held.load(Ordering::SeqCst) {
                self.passed_over.set(self.passed_over.get() + 1);
                if self.passed_over.get() >= 2 {
                    self.ask_looper();
                }
            } else {
                self.passed_over.set(0);
            }
        } else {
            // Nothing to draw, and nothing waiting to be shown: what
            // the looper kept for this frame is asked for, and its
            // commit drawn at once.
            self.ask_looper();
        }
        drew
    }

    /// Asks the looper for what it kept for a frame, if anything.
    fn ask_looper(&self) {
        self.passed_over.set(0);
        if self.pace.held.swap(false, Ordering::SeqCst) {
            self.link.send(WindowEvent::FrameDue);
            self.asked.set(Some(Instant::now()));
        }
    }

    fn draw_frame(&self, options: RequestFrameOptions) -> bool {
        let started = Instant::now();
        let mut showing = self.showing.borrow_mut();
        let showing = &mut *showing;
        if options.force_render {
            showing.dirty = true;
            showing.damage.clear();
        }
        let Some(commit) = &showing.commit else { return false };
        // A size the looper hasn't laid out for yet: its commit for it is
        // on its way, and draws then. (Unless it's taking too long.)
        if let Some(since) = showing.resized_at {
            if since.elapsed() < SIZE_WAIT {
                return false;
            }
            showing.dirty = true;
            showing.damage.clear();
        }
        let animating = commit.animating;
        // Where each scroller's content shows now: where it's scrolled
        // to, and past an end as far as that's stretched (springing
        // back: it moves by itself, frame by frame).
        let mut springing = false;
        if !showing.scrollers.is_empty() {
            let mut moved: SmallVec<[Bounds<ScaledPixels>; 2]> = SmallVec::new();
            let mut told = false;
            let places = layer_places(&commit.layers, &showing.scrollers, commit.scale);
            for (i, layer) in commit.layers.iter().enumerate().skip(1) {
                let Some(scroller) = showing.scrollers.get_mut(&layer.id) else { continue };
                let (shown, moving) = scroller.elastic.offset(scroller.offset);
                springing |= moving;
                // Its view: what it showed there, and what it shows now.
                // (Drawn while it springs though it's moved less than a
                // device pixel: the frames it's paced by are the ones
                // drawn, and it's a handful at the end.)
                if moving || device(shown, commit.scale) != device(scroller.shown, commit.scale) {
                    moved.push(places[i].1);
                }
                told |= shown != scroller.shown;
                scroller.shown = shown;
            }
            for rect in moved {
                add_damage(&mut showing.dirty, &mut showing.damage, rect);
            }
            if told {
                self.report(&showing.scrollers);
            }
        }
        // The looper's answer to input just sent is on its way: what
        // there is to draw is drawn with it ([`INPUT_HOLD`]).
        if showing.dirty && !options.force_render && showing.resized_at.is_none() && self.answering() {
            let waited = self.input_at.get().elapsed();
            if waited < INPUT_HOLD {
                // (Drawn when the hold is over whether or not the looper
                // has answered: nothing else may ask for a frame by then.)
                if !self.pace.holding.swap(true, Ordering::SeqCst) {
                    if let Some(window) = self.window.borrow().as_ref() {
                        window.frame_after(INPUT_HOLD - waited);
                    }
                }
                if crate::stats::frame_phases() {
                    eprintln!("frame kept back: input {} sent, {} answered, {:.2} ms ago", self.sent.get(), self.answered.get(), waited.as_secs_f64() * 1000.0);
                }
                return false;
            }
            // (Too long: what it answers when it does is another frame's.)
            self.answered.set(self.sent.get());
            self.pace.holding.store(false, Ordering::SeqCst);
        }
        if !showing.dirty {
            // (Commits that changed nothing to draw made no frame: the
            // next one that does doesn't take their work for its own.)
            showing.work = Duration::ZERO;
            if animating {
                self.link.send(WindowEvent::Frame);
            }
            return false;
        }
        // What changed, as rectangles (none: all of it), and how much of
        // the image the renderer hands out has to be drawn for it: an
        // image keeps what it last held, so most frames draw a small
        // part of the window, or a few apart (a line typed in and the
        // status line under it), and tell app_server only those.
        let changed = (!showing.damage.is_empty()).then(|| std::mem::take(&mut showing.damage));
        let work = std::mem::take(&mut showing.work);
        {
            let window = self.window.borrow();
            let Some(window) = window.as_ref() else { return false };
            let part = match window.begin_frame(changed.as_deref()) {
                FrameRegion::None => return false,
                FrameRegion::All => None,
                FrameRegion::Part(part) => Some(part),
            };
            let begun = started.elapsed();
            let scene = &mut showing.scene;
            scene.clear();
            assemble(commit, &showing.scrollers, scene, part.as_deref());
            scene.finish();
            let assembled = started.elapsed();
            window.draw(scene);
            let drawn = started.elapsed();
            window.completed_frame();
            if crate::stats::frame_phases() {
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                let part = match &part {
                    Some(parts) if parts.is_empty() => "nothing".to_owned(),
                    Some(parts) => parts.iter().map(|p| format!("{}×{} at {},{}", p.size.width.0, p.size.height.0, p.origin.x.0, p.origin.y.0)).collect::<Vec<_>>().join(" + "),
                    None => "all".to_owned(),
                };
                // (How many draws it was: things of a kind at one order
                // are one, so a frame of rows drawn row by row, each
                // above the last, is hundreds where it could be ten.)
                let batches = scene.batches().count();
                eprintln!(
                    "frame {:.3} ms: begin {:.3}, assemble {:.3}, draw {:.3}, complete {:.3}; {part}; {} of {} items, {} sprites, {} quads, {batches} batches; looper {:.3} ms",
                    ms(started.elapsed()),
                    ms(begun),
                    ms(assembled - begun),
                    ms(drawn - assembled),
                    ms(started.elapsed() - drawn),
                    scene.len(),
                    commit.items.len(),
                    scene.monochrome_sprites.len() + scene.polychrome_sprites.len(),
                    scene.quads.len(),
                    ms(work),
                );
                if let Some(made) = self.made.take() {
                    eprintln!("first frame drawn {:.3} ms after its window was made", ms(made.elapsed()));
                }
            }
        }
        showing.dirty = false;
        showing.damage.clear();
        let whole = work + started.elapsed();
        // Where each scroller showed its content in this frame, and where
        // what scrolls itself said it was (none: no allocation). How far
        // it's scrolled, across and down, and the furthest it goes.
        let scrolls = match &showing.commit {
            Some(commit) if !showing.scrollers.is_empty() || !commit.noted.is_empty() => commit
                .layers
                .iter()
                .filter_map(|layer| showing.scrollers.get(&layer.id).map(|s| [-s.shown.x.0, -s.shown.y.0, layer.max.width.0, layer.max.height.0]))
                .chain(commit.noted.iter().copied())
                .collect(),
            _ => Vec::new(),
        };
        // (Marked when it was drawn, not when what led to it began: how
        // evenly frames come is read from these.)
        showing.stats.record(started, work, whole, scrolls);
        if animating {
            self.link.send(WindowEvent::Frame);
        }
        if springing {
            // (An end springing back: the next frame moves it on.)
            let waker = self.waker.borrow().clone();
            if let Some(waker) = waker {
                waker();
            }
        }
        true
    }

    /// The frames drawn since last asked (`{"frames":true}`).
    pub(crate) fn take_frame_stats(&self) -> FrameStats {
        std::mem::take(&mut self.showing.borrow_mut().stats)
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        self.waker.borrow_mut().take();
    }
}

impl Scroller {
    /// Where its content shows now.
    fn shown_now(&self) -> Point<Pixels> {
        self.elastic.clone().offset(self.offset).0
    }
}

/// An offset in whole device pixels (what's drawn is moved by whole
/// ones: nothing is drawn again for being scrolled).
fn device(offset: Point<Pixels>, scale: f32) -> Point<ScaledPixels> {
    point(ScaledPixels((offset.x.0 * scale).round()), ScaledPixels((offset.y.0 * scale).round()))
}

const EVERYTHING: Bounds<ScaledPixels> = Bounds { origin: Point { x: ScaledPixels(-1.0e7), y: ScaledPixels(-1.0e7) }, size: Size { width: ScaledPixels(2.0e7), height: ScaledPixels(2.0e7) } };

/// Where each layer's content is now (how far from where it was
/// painted), and what clips it, in the window's device pixels.
fn layer_places(layers: &[Layer], scrollers: &FxHashMap<u64, Scroller>, scale: f32) -> Vec<(Point<ScaledPixels>, Bounds<ScaledPixels>)> {
    let nowhere = point(ScaledPixels(0.0), ScaledPixels(0.0));
    let mut places: Vec<(Point<ScaledPixels>, Bounds<ScaledPixels>)> = Vec::with_capacity(layers.len().max(1));
    for (i, layer) in layers.iter().enumerate() {
        if i == 0 {
            places.push((nowhere, EVERYTHING));
            continue;
        }
        // (A layer comes after the one it's in.)
        let (parent_shift, parent_clip) = places.get(layer.parent as usize).copied().unwrap_or((nowhere, EVERYTHING));
        let viewport = Bounds { origin: layer.viewport.origin + parent_shift, size: layer.viewport.size };
        let offset = scrollers.get(&layer.id).map(|s| device(s.shown, scale)).unwrap_or_default();
        places.push((parent_shift + offset, viewport.intersect(&parent_clip)));
    }
    if places.is_empty() {
        places.push((nowhere, EVERYTHING));
    }
    places
}

/// Puts a frame together from what's committed: all of it, or only
/// what shows in `parts` (each drawn under a scissor of its own): a
/// drawing in none is left out, and one in some is cut to those (a text
/// area typed in gives the frame its line, not all its glyphs).
fn assemble(commit: &Commit, scrollers: &FxHashMap<u64, Scroller>, scene: &mut Scene, parts: Option<&[Bounds<ScaledPixels>]>) {
    let places = layer_places(&commit.layers, scrollers, commit.scale);
    for item in &commit.items {
        let (shift, layer_clip) = places.get(item.layer as usize).copied().unwrap_or(places[0]);
        let by = item.by + shift;
        let mut clip = Bounds { origin: item.clip.origin + shift, size: item.clip.size }.intersect(&layer_clip);
        if let Some(parts) = parts {
            let reach = Bounds { origin: item.list.bounds.origin + by, size: item.list.bounds.size }.intersect(&clip);
            match parts.iter().filter(|p| !reach.intersect(p).is_empty()).copied().reduce(|a, b| a.union(&b)) {
                Some(hit) => clip = clip.intersect(&hit),
                None => continue,
            }
        }
        if clip.is_empty() {
            continue;
        }
        let reach = Bounds { origin: item.list.bounds.origin + by, size: item.list.bounds.size };
        if reach.intersect(&clip).is_empty() {
            continue;
        }
        scene.append(&item.list.scene, by, &clip);
    }
}

/// What differs between two commits of one size: where anything was or
/// is that isn't the same drawing in the same place.
fn damage(before: &Commit, now: &Commit, scrollers: &FxHashMap<u64, Scroller>, into: &mut Vec<Bounds<ScaledPixels>>) {
    let whole = Bounds { origin: point(ScaledPixels(0.0), ScaledPixels(0.0)), size: size(ScaledPixels(now.size.width.0 * now.scale), ScaledPixels(now.size.height.0 * now.scale)) };
    let same_layers = before.layers.len() == now.layers.len() && before.layers.iter().zip(&now.layers).all(|(a, b)| a.id == b.id && a.parent == b.parent);
    if !same_layers {
        // (What scrolls changed: all of it.)
        into.clear();
        into.push(whole);
        return;
    }
    let places = layer_places(&now.layers, scrollers, now.scale);
    // A scroller's view moved or changed size: where it was and is.
    if before.layers.iter().zip(&now.layers).any(|(a, b)| a.viewport != b.viewport) {
        let was = layer_places(&before.layers, scrollers, before.scale);
        for (i, (a, b)) in before.layers.iter().zip(&now.layers).enumerate() {
            if a.viewport != b.viewport {
                into.extend([was[i].1, places[i].1].into_iter().filter(|r| !r.is_empty()).map(|r| r.intersect(&whole)));
            }
        }
    }
    let (a, b) = (&before.items, &now.items);
    // Most of a window is the same from commit to commit, and in the
    // same order: what's the same at each end is passed over.
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start].same_place(&b[start]) {
        start += 1;
    }
    let (mut end_a, mut end_b) = (a.len(), b.len());
    while end_a > start && end_b > start && a[end_a - 1].same_place(&b[end_b - 1]) {
        end_a -= 1;
        end_b -= 1;
    }
    let (a, b) = (&a[start..end_a], &b[start..end_b]);
    if a.is_empty() && b.is_empty() {
        return;
    }
    // Part `r` of an item's drawing (where it was painted), where it
    // shows in the window.
    let place = |item: &Item, r: Bounds<ScaledPixels>| {
        let (shift, clip) = places.get(item.layer as usize).copied().unwrap_or(places[0]);
        let shown = Bounds { origin: r.origin + item.by, size: r.size }.intersect(&item.clip);
        Bounds { origin: shown.origin + shift, size: shown.size }.intersect(&clip)
    };
    let mut add = |r: Bounds<ScaledPixels>| {
        if !r.is_empty() {
            into.push(r);
        }
    };
    // A drawing worth comparing thing by thing with the one it replaces
    // (smaller ones are drawn again whole: cheaper than looking).
    let big = |item: &Item| {
        let shown = item.shown();
        shown.size.width.0 * shown.size.height.0 * 16.0 >= whole.size.width.0 * whole.size.height.0
    };
    // In between, what's in both in the same place and the same order
    // didn't change; the rest did, where it was and where it is.
    let mut j = 0;
    let mut differ = Vec::new();
    for (i, item) in a.iter().enumerate() {
        match b[j..].iter().position(|other| other.same_place(item)) {
            Some(at) => {
                b[j..j + at].iter().for_each(|x| add(place(x, x.list.bounds)));
                j += at + 1;
            }
            None => {
                // Painted again where it was (a text area typed in: a
                // line and the caret changed, not all of it), and not
                // what a later one is: only where the two differ.
                if let Some(next) = b.get(j)
                    && next.by == item.by
                    && next.clip == item.clip
                    && next.layer == item.layer
                    && big(item)
                    && !a[i + 1..].iter().any(|later| later.same_place(next))
                {
                    differ.clear();
                    if next.list.scene.changed_since(&item.list.scene, &mut differ) {
                        differ.iter().for_each(|r| add(place(item, *r)));
                        j += 1;
                        continue;
                    }
                }
                add(place(item, item.list.bounds));
            }
        }
    }
    b[j..].iter().for_each(|x| add(place(x, x.list.bounds)));
    // (Many small pieces cost more to draw apart than together.)
    const MOST: usize = 16;
    if into.len() > MOST {
        let all = into.iter().copied().reduce(|a, b| a.union(&b)).unwrap_or_default();
        into.clear();
        into.push(all);
    }
}
