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
    rc::{Rc, Weak},
    sync::Arc,
    time::{Duration, Instant},
};

use olo_app::{Latest, LooperHandle};

use crate::{
    Bounds, FrameRegion, FrameStats, GpuSpecs, ImeInput, Pixels, PlatformAtlas, PlatformInput, PlatformWindow, Point, RequestFrameOptions, ScaledPixels, Scene, Size, TextRendering, WindowId, point,
    px, size,
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
/// display thread keeps (so it can move it without the looper).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Layer {
    /// The same from commit to commit for the same scroll view (0: the
    /// window itself, which doesn't scroll).
    pub(crate) id: u64,
    /// The layer it's in.
    pub(crate) parent: u32,
    /// Where it shows, in its parent's coordinates.
    pub(crate) viewport: Bounds<ScaledPixels>,
    /// How far its content reaches past the viewport, each way.
    pub(crate) range: Size<ScaledPixels>,
    /// Where the looper put it, if it moved it (a view scrolled to):
    /// otherwise the display thread's own stands.
    pub(crate) offset: Option<Point<ScaledPixels>>,
}

/// What a window shows now, from its looper.
pub(crate) struct Commit {
    /// The size and scale it was laid out for.
    pub(crate) size: Size<Pixels>,
    pub(crate) scale: f32,
    /// Every recording that shows, in painting order.
    pub(crate) items: Vec<Item>,
    pub(crate) layers: Vec<Layer>,
    /// When the looper took what led to this (input), and how long it
    /// worked on it: for the frame's times.
    pub(crate) started: Instant,
    pub(crate) work: Duration,
    /// Everything is to be drawn again whatever matches (the look or the
    /// text's rendering changed what the same drawings mean).
    pub(crate) everything: bool,
    /// The looper animates: it wants [`WindowEvent::Frame`] as the next
    /// frame starts.
    pub(crate) animating: bool,
}

/// What the display thread tells a window's looper.
#[derive(Debug)]
pub(crate) enum WindowEvent {
    Input(PlatformInput),
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
    /// The display it's on, in points.
    Display(Option<Bounds<Pixels>>),
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
    pub(crate) size: Size<Pixels>,
    pub(crate) scale: f32,
    pub(crate) atlas: Arc<dyn PlatformAtlas>,
    pub(crate) text_rendering: TextRendering,
    pub(crate) surface_id: Option<u32>,
    pub(crate) gpu: Option<GpuSpecs>,
    pub(crate) display: Option<Bounds<Pixels>>,
}

/// How long a new size waits for the looper to lay out for it before
/// the old layout is drawn at it (a looper that's stuck doesn't keep the
/// window from following the person's hand).
const SIZE_WAIT: Duration = Duration::from_millis(250);

struct Showing {
    commit: Option<Commit>,
    /// Something changed since the last frame drawn.
    dirty: bool,
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

pub(crate) struct Stage {
    id: WindowId,
    window: RefCell<Option<Box<dyn PlatformWindow>>>,
    link: WindowLink,
    showing: RefCell<Showing>,
    waker: RefCell<Option<Rc<dyn Fn()>>>,
    /// Pointer motion not yet taken by the looper: the latest stands for
    /// all of it.
    motion: RefCell<Latest<PlatformInput>>,
    closed: Cell<bool>,
}

impl Stage {
    pub(crate) fn new(id: WindowId, window: Box<dyn PlatformWindow>, link: WindowLink) -> (Rc<Stage>, Opened) {
        let size = window.content_size();
        let scale = window.scale_factor();
        let opened = Opened {
            size,
            scale,
            atlas: window.sprite_atlas(),
            text_rendering: window.text_rendering(),
            surface_id: window.surface_id(),
            gpu: window.gpu_specs(),
            display: window.display().map(|d| d.bounds()),
        };
        let waker = window.frame_waker();
        let stage = Rc::new(Stage {
            id,
            window: RefCell::new(None),
            link,
            showing: RefCell::new(Showing { commit: None, dirty: false, damage: Vec::new(), size, scale, resized_at: Some(Instant::now()), stats: FrameStats::default(), scene: Scene::default() }),
            waker: RefCell::new(waker),
            motion: RefCell::new(Latest::new()),
            closed: Cell::new(false),
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

    /// The next motion starts afresh: something else was sent, and
    /// motion from before it mustn't be taken for motion after.
    fn break_motion(&self) {
        *self.motion.borrow_mut() = Latest::new();
    }

    fn input(&self, input: PlatformInput) {
        let id = self.id;
        match input {
            PlatformInput::MouseMove(_) => {
                let motion = self.motion.borrow().clone();
                motion.put(&self.link.looper, input, move |looper, input| crate::window::deliver(looper, id, WindowEvent::Input(input)));
            }
            other => {
                self.break_motion();
                self.link.send(WindowEvent::Input(other));
            }
        }
    }

    fn resized(&self, size: Size<Pixels>, scale: f32) {
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
        {
            let mut showing = self.showing.borrow_mut();
            let showing = &mut *showing;
            match (&showing.commit, commit.everything) {
                (Some(before), false) if before.scale == commit.scale && before.size == commit.size => {
                    let all = showing.dirty && showing.damage.is_empty();
                    if !all {
                        damage(before, &commit, &mut showing.damage);
                        showing.dirty |= !showing.damage.is_empty();
                    }
                }
                _ => {
                    showing.dirty = true;
                    showing.damage.clear();
                }
            }
            if commit.size == showing.size && commit.scale == showing.scale {
                showing.resized_at = None;
            }
            showing.commit = Some(commit);
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
        if !showing.dirty {
            if animating {
                self.link.send(WindowEvent::Frame);
            }
            return false;
        }
        // What changed, as one rectangle (None: all of it), and how much
        // of the image the renderer hands out has to be drawn for it: an
        // image keeps what it last held, so most frames draw a small
        // part of the window, and tell app_server only that changed.
        let changed = showing.damage.iter().copied().reduce(|a, b| a.union(&b));
        let work = commit.work;
        let began = commit.started;
        {
            let window = self.window.borrow();
            let Some(window) = window.as_ref() else { return false };
            let part = match window.begin_frame(changed) {
                FrameRegion::None => return false,
                FrameRegion::All => None,
                FrameRegion::Part(part) => Some(part),
            };
            let scene = &mut showing.scene;
            scene.clear();
            assemble(commit, scene, part);
            scene.finish();
            let assembled = started.elapsed();
            window.draw(scene);
            window.completed_frame();
            if crate::stats::frame_phases() {
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                let part = match part {
                    Some(p) => format!("{}×{} at {},{}", p.size.width.0, p.size.height.0, p.origin.x.0, p.origin.y.0),
                    None => "all".to_owned(),
                };
                eprintln!(
                    "frame {:.3} ms: assemble {:.3}, draw {:.3}; {part}; {} of {} items, {} sprites, {} quads; looper {:.3} ms",
                    ms(started.elapsed()),
                    ms(assembled),
                    ms(started.elapsed() - assembled),
                    scene.len(),
                    commit.items.len(),
                    scene.monochrome_sprites.len() + scene.polychrome_sprites.len(),
                    scene.quads.len(),
                    ms(work),
                );
            }
        }
        showing.dirty = false;
        showing.damage.clear();
        let whole = work + started.elapsed();
        showing.stats.record(began, work, whole, Vec::new());
        if animating {
            self.link.send(WindowEvent::Frame);
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

/// Where each layer's content is now, and what clips it, in the
/// window's device pixels.
fn layer_places(layers: &[Layer]) -> Vec<(Point<ScaledPixels>, Bounds<ScaledPixels>)> {
    let everything = Bounds { origin: point(ScaledPixels(-1.0e7), ScaledPixels(-1.0e7)), size: size(ScaledPixels(2.0e7), ScaledPixels(2.0e7)) };
    let mut places: Vec<(Point<ScaledPixels>, Bounds<ScaledPixels>)> = Vec::with_capacity(layers.len().max(1));
    for (i, layer) in layers.iter().enumerate() {
        if i == 0 {
            places.push((point(ScaledPixels(0.0), ScaledPixels(0.0)), everything));
            continue;
        }
        // (A layer comes after the one it's in.)
        let (parent_shift, parent_clip) = places.get(layer.parent as usize).copied().unwrap_or((point(ScaledPixels(0.0), ScaledPixels(0.0)), everything));
        let viewport = Bounds { origin: layer.viewport.origin + parent_shift, size: layer.viewport.size };
        let offset = layer.offset.unwrap_or_default();
        places.push((point(parent_shift.x - offset.x, parent_shift.y - offset.y), viewport.intersect(&parent_clip)));
    }
    if places.is_empty() {
        places.push((point(ScaledPixels(0.0), ScaledPixels(0.0)), everything));
    }
    places
}

/// Puts a frame together from what's committed: all of it, or only
/// what shows in `part`.
fn assemble(commit: &Commit, scene: &mut Scene, part: Option<Bounds<ScaledPixels>>) {
    let places = layer_places(&commit.layers);
    for item in &commit.items {
        let (shift, layer_clip) = places.get(item.layer as usize).copied().unwrap_or(places[0]);
        let by = item.by + shift;
        let mut clip = Bounds { origin: item.clip.origin + shift, size: item.clip.size }.intersect(&layer_clip);
        if let Some(part) = &part {
            clip = clip.intersect(part);
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
fn damage(before: &Commit, now: &Commit, into: &mut Vec<Bounds<ScaledPixels>>) {
    if before.layers != now.layers {
        // (What scrolls changed: all of it, for now.)
        into.clear();
        into.push(Bounds { origin: point(ScaledPixels(0.0), ScaledPixels(0.0)), size: size(ScaledPixels(now.size.width.0 * now.scale), ScaledPixels(now.size.height.0 * now.scale)) });
        return;
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
    let places = layer_places(&now.layers);
    let mut add = |item: &Item| {
        let (shift, clip) = places.get(item.layer as usize).copied().unwrap_or(places[0]);
        let shown = item.shown();
        let shown = Bounds { origin: shown.origin + shift, size: shown.size }.intersect(&clip);
        if !shown.is_empty() {
            into.push(shown);
        }
    };
    // In between, what's in both in the same place and the same order
    // didn't change; the rest did, where it was and where it is.
    let mut j = 0;
    for item in a {
        match b[j..].iter().position(|other| other.same_place(item)) {
            Some(at) => {
                b[j..j + at].iter().for_each(&mut add);
                j += at + 1;
            }
            None => add(item),
        }
    }
    b[j..].iter().for_each(&mut add);
    // (Many small pieces cost more to draw apart than together.)
    const MOST: usize = 16;
    if into.len() > MOST {
        let all = into.iter().copied().reduce(|a, b| a.union(&b)).unwrap_or_default();
        into.clear();
        into.push(all);
    }
    let _ = px;
}
