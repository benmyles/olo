//! A window, on its looper's side: its views, its tree, and what it
//! does between being told something and showing what came of it.
//!
//! BeOS's `BWindow` was a looper with a tree of views. So is this. What
//! the window is told (input from the display thread, messages for its
//! views, timers) is taken one at a time; when the queue is dry the
//! window is *flushed*: views that changed say what they show now, the
//! tree is brought in line, what could have moved is laid out, what
//! changed is drawn, and the lot is committed to the window's stage on
//! the display thread, which shows it. A burst of messages is one
//! flush.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use collections::FxHashMap;
use olo_app::Looper;
use slotmap::SlotMap;

use crate::{
    AvailableSpace, Bounds, Modifiers, MouseDownEvent, Pixels, PlatformWindow, Point, ScaledPixels, SharedString, Size, SvgRenderer, TextRendering, TextStyle, WindowId, WindowParams,
    WindowTextSystem,
    display::display,
    layout::Layout,
    painter::Gfx,
    point, px, size,
    stage::{Commit, Item, Layer, Opened, ScrollReport, WheelKind, WheelRegion, WindowEvent, WindowLink, WindowState},
    tree::{Node, NodeId, NodeKind, ViewId},
    view::{AnyView, Child, Cx, View, ViewSlot},
};

thread_local! {
    static TEXT: std::cell::RefCell<Option<Arc<WindowTextSystem>>> = const { std::cell::RefCell::new(None) };
    static SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
    static SIZE: std::cell::Cell<Size<Pixels>> = const { std::cell::Cell::new(Size { width: Pixels(0.), height: Pixels(0.) }) };
    /// Something asked the window's size since this was last cleared (a
    /// view's `body`, or what it built: it says what it shows again
    /// when the window is sized).
    static SIZE_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static ACTIVE_READ: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The size of the window that's being worked on now, on this thread
/// (see [`scale_factor`]): its content, in points. A view whose `body`
/// asks (or builds something that does: a tool bar as long as its
/// window) says what it shows again when the window is sized.
pub fn viewport_size() -> Size<Pixels> {
    SIZE_READ.with(|r| r.set(true));
    SIZE.with(|s| s.get())
}

/// The scale of the window that's being worked on now, on this thread
/// (the one whose view is saying what it shows, or whose handler is
/// running): device pixels to a point. For what's built without the
/// window to hand (an element sized in whole device pixels). A window
/// whose scale changes has every view say what it shows again.
pub fn scale_factor() -> f32 {
    SCALE.with(|s| s.get())
}

/// The calling thread's text system: the program's fonts and glyphs
/// with this thread's own cache of shaped lines (every window on a
/// looper shares it, and so does text measured before anything is
/// drawn: a button sized for its label finds the label shaped already
/// when it draws it).
pub fn thread_text_system() -> Arc<WindowTextSystem> {
    TEXT.with(|t| t.borrow_mut().get_or_insert_with(|| Arc::new(WindowTextSystem::new(display().text_system.clone()))).clone())
}

/// The windows on a looper (most loopers have one; a window's menus and
/// tips are windows on its looper too).
#[derive(Default)]
pub(crate) struct Windows {
    map: HashMap<WindowId, Box<WindowCore>>,
    hooked: bool,
}

/// Something to do when the next frame starts.
type NextFrame = Box<dyn FnOnce(&mut WindowCx)>;

pub(crate) struct WindowCore {
    pub(crate) id: WindowId,
    pub(crate) nodes: SlotMap<NodeId, Node>,
    pub(crate) views: SlotMap<ViewId, ViewSlot>,
    pub(crate) layout: Layout<NodeId>,
    pub(crate) root: Option<NodeId>,
    pub(crate) root_view: Option<ViewId>,
    /// The view the program knows the window by, if it isn't its first
    /// (a frame round it): what the window's handle reaches, and what
    /// is asked whether the window may close.
    pub(crate) main_view: Option<ViewId>,
    /// How many times it has committed (told the stage what it shows).
    pub(crate) commits: u64,
    /// The window it belongs to (a menu's, a tip's): it closes with it.
    pub(crate) owner: Option<WindowId>,
    /// A surface of the desktop's own (a layer's, the lock screen's):
    /// it stays with its display.
    pub(crate) shell: bool,
    /// A menu (a popup that takes the keyboard): its owner is as active
    /// as it was while this is open ([`WindowCx::is_window_active`]).
    pub(crate) menu: bool,
    pub(crate) gfx: Gfx,
    /// The window's content, in points.
    pub(crate) size: Size<Pixels>,
    pub(crate) title: SharedString,
    pub(crate) surface_id: Option<u32>,
    /// The display it's on, in points, if that's known.
    pub(crate) display: Option<Bounds<Pixels>>,
    pub(crate) dirty_views: Vec<ViewId>,
    pub(crate) paint_queue: Vec<NodeId>,
    /// Texts measured anew in the layout under way: drawn again.
    pub(crate) remeasured: Vec<NodeId>,
    /// Something that shows changed: there's a commit to make.
    pub(crate) needs_commit: bool,
    /// Everything that shows, in painting order, as last committed, and
    /// whether it's to be made again (what shows or where changed, not
    /// only what something drew).
    pub(crate) items: Vec<Item>,
    pub(crate) items_stale: bool,
    /// What scrolls, as last committed: the layers (the window first),
    /// where the wheel means something, each scroller's node, and what
    /// is drawn by a scroller (its bars).
    pub(crate) layers: Vec<Layer>,
    pub(crate) wheel: Vec<WheelRegion>,
    pub(crate) scrollers: FxHashMap<u64, NodeId>,
    pub(crate) watchers: Vec<(u64, u8, NodeId)>,
    /// Scrollers that moved (and which ways), whose listeners and watchers haven't heard.
    pub(crate) scroll_changed: Vec<(u64, u8)>,
    /// Scrollers asked to go somewhere once layout is done.
    pub(crate) scroll_asks: Vec<crate::ScrollHandle>,
    /// What the pointer is over may have changed under it (something
    /// scrolled).
    pub(crate) hover_stale: bool,
    /// An element that counts the keyboard as its own while it's inside
    /// (`focus_within`: a text control's well) was made anew: whether
    /// it's within it is found out (`flush`), not only when the keyboard
    /// next moves. (A panel laid out again round a field being typed in,
    /// a box gaining its Reset: the field's ring went.)
    pub(crate) within_stale: bool,
    /// The timers of views that ended (`Cx::every`'s), to be cancelled
    /// once there's a looper to hand (`with_window`): a caret's blink
    /// went on waking the window twice a second after its field's dialog
    /// had closed.
    pub(crate) ended_timers: Vec<olo_app::TimerId>,
    /// Nodes drawn by what can be seen of them.
    pub(crate) by_visible: Vec<NodeId>,
    /// The nodes whose drawings place another program's pictures
    /// (`Painter::place_external`), as they were last drawn.
    pub(crate) with_externals: Vec<NodeId>,
    /// Nodes that hold rows made as they come near what shows.
    pub(crate) row_nodes: Vec<NodeId>,
    /// Nodes layout moved or sized, whose listeners haven't heard.
    pub(crate) bounds_changed: Vec<NodeId>,
    /// Nothing is shown until then (`WindowCx::hold_commit`).
    pub(crate) commit_held: Option<Instant>,
    /// How app_server holds the window.
    pub(crate) state: WindowState,
    /// Layout is to be worked out again though nothing in the tree says
    /// so (the window's size changed).
    pub(crate) relayout: bool,
    /// The next commit is to be drawn whole.
    pub(crate) everything: bool,
    /// When what's being worked on now was first taken.
    pub(crate) started: Option<Instant>,
    pub(crate) mouse: Point<Pixels>,
    /// The pointer is over the window (or files dragged from elsewhere
    /// are): until it has come, and once it has left, nothing in the
    /// window is under it, wherever it last was.
    pub(crate) pointer_in: bool,
    pub(crate) modifiers: Modifiers,
    /// What the pointer is over, topmost first.
    pub(crate) hovered: Vec<NodeId>,
    /// Files being dragged over the window, from somewhere else: handed
    /// to what they're dropped on (`InteractiveElement::on_drop`).
    pub(crate) dropping: Option<crate::ExternalPaths>,
    /// What's dragged over the window now (its types and what its
    /// source allows), and what the display thread was last told the
    /// window takes it as (`View::drop_over`).
    pub(crate) drop_offer: Option<crate::DropOffer>,
    pub(crate) drop_accept_sent: Option<Option<crate::DropAccept>>,
    /// What a button went down on, until it's let go.
    pub(crate) pressed: Vec<(NodeId, MouseDownEvent)>,
    pub(crate) focus: Option<NodeId>,
    /// A view to be given the keyboard once it's in the tree.
    pub(crate) pending_focus: Option<ViewId>,
    /// An element (by its focus handle) to be given it once it is.
    pub(crate) pending_focus_handle: Option<u64>,
    /// An element to be given it once it's shown, if nothing has it
    /// then.
    pub(crate) fallback_focus: Option<u64>,
    /// Nodes that hear the pointer anywhere in the window.
    pub(crate) window_listeners: Vec<NodeId>,
    /// The window has the keyboard.
    pub(crate) active: bool,
    pub(crate) closed: bool,
    /// What's being handed round now may go on to the next that would
    /// take it, and what would happen by default may.
    pub(crate) propagate: bool,
    pub(crate) default_prevented: bool,
    /// The node whose handler is running now.
    pub(crate) current: Option<NodeId>,
    pub(crate) next_frame: Vec<NextFrame>,
    /// Views invalidated for the next frame (`Cx::invalidate_at_frame`),
    /// kept while the stage waits to draw one, and since when.
    pub(crate) at_frame: Vec<ViewId>,
    pub(crate) held_since: Option<Instant>,
    pub(crate) pace: Arc<crate::stage::FramePace>,
    /// When what moves with time is drawn again.
    pub(crate) clock: Clock,
    /// The pointer's shape, as last asked for.
    pub(crate) cursor: crate::CursorStyle,
    /// Scenes let go of, to record into again.
    pub(crate) recycled: Vec<crate::Scene>,
    /// What the kit keeps with the window, one of each type.
    pub(crate) kept: HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
    /// What elements keep with the window by name (`keyed_state`).
    pub(crate) keyed: HashMap<(std::any::TypeId, SharedString), std::rc::Rc<dyn std::any::Any>>,
}

/// What `list_items` makes.
struct Listing {
    items: Vec<Item>,
    layers: Vec<Layer>,
    wheel: Vec<WheelRegion>,
    scrollers: FxHashMap<u64, NodeId>,
    watchers: Vec<(u64, u8, NodeId)>,
    /// What's raised (`InteractiveElement::raised`), found on the way and
    /// listed after everything else; None while they're being listed.
    raised: Option<Vec<(NodeId, u32)>>,
}

/// No clip at all, in device pixels.
/// A node's place in the list of what shows when the list doesn't reach
/// it (what holds it is hidden, or cut away whole): it shows nothing,
/// whatever it draws next.
const UNLISTED: u32 = u32::MAX - 1;

const EVERYWHERE: Bounds<ScaledPixels> = Bounds { origin: Point { x: ScaledPixels(-1.0e7), y: ScaledPixels(-1.0e7) }, size: Size { width: ScaledPixels(2.0e7), height: ScaledPixels(2.0e7) } };

/// A window, as whatever is running on its looper is handed it: the
/// window's own state and the looper.
pub struct WindowCx<'a> {
    pub(crate) core: &'a mut WindowCore,
    pub(crate) looper: &'a mut Looper,
}

/// An element in a window's tree, by name: something to draw again
/// (`WindowCx::repaint_node`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef(pub(crate) NodeId);

/// A window on the calling looper, as another window there knows it
/// (the owner of a menu or a tip): reached at once, not by a message.
pub struct LocalWindow<V> {
    id: WindowId,
    _view: std::marker::PhantomData<fn() -> V>,
}

impl<V> Clone for LocalWindow<V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<V> Copy for LocalWindow<V> {}

impl<V: View> LocalWindow<V> {
    pub fn id(&self) -> WindowId {
        self.id
    }

    /// The same window, its own view taken to be a `T` (what
    /// `WindowCx::set_main_view` named: a window whose first view is a
    /// frame round the view its opener knows it by).
    pub fn retyped<T: View>(self) -> LocalWindow<T> {
        LocalWindow { id: self.id, _view: std::marker::PhantomData }
    }

    /// The window `cx` is, whose own view is a `V`: for it to hand to a
    /// window it opens (a submenu, which closes its menu with it).
    pub fn of(cx: &WindowCx) -> LocalWindow<V> {
        LocalWindow { id: cx.core.id, _view: std::marker::PhantomData }
    }

    /// Does something with the window. None if it has closed.
    pub fn with<R>(&self, cx: &mut WindowCx, f: impl FnOnce(&mut WindowCx) -> R) -> Option<R> {
        with_window(cx.looper, self.id, f)
    }

    /// Reaches into the window's own view. None if it has closed.
    pub fn update<R>(&self, cx: &mut WindowCx, f: impl FnOnce(&mut V, &mut Cx<V>) -> R) -> Option<R> {
        with_window(cx.looper, self.id, |window| {
            let view = window.core.main_view.or(window.core.root_view)?;
            window.update_view(view, f)
        })
        .flatten()
    }

    pub fn close(&self, cx: &mut WindowCx) {
        self.with(cx, |window| window.close_window());
    }

    /// Whether it's still open.
    pub fn is_open(&self, cx: &WindowCx) -> bool {
        cx.looper.state::<Windows>().is_some_and(|w| w.map.contains_key(&self.id))
    }
}

/// Does something with a window on this looper (not from inside
/// itself: a window that's taking something is out, and None comes
/// back).
pub(crate) fn with_window<R>(looper: &mut Looper, id: WindowId, f: impl FnOnce(&mut WindowCx) -> R) -> Option<R> {
    let mut core = looper.state_mut::<Windows>()?.map.remove(&id)?;
    let outer = (SCALE.with(|s| s.replace(core.gfx.scale_factor)), SIZE.with(|s| s.replace(core.size)));
    let result = f(&mut WindowCx { core: &mut core, looper });
    SCALE.with(|s| s.set(outer.0));
    SIZE.with(|s| s.set(outer.1));
    for timer in core.ended_timers.drain(..) {
        looper.cancel(timer);
    }
    if core.closed {
        // Its own view hears it's going, while there's still a window
        // to hear it in.
        let mut cx = WindowCx { core: &mut core, looper };
        for view in [cx.core.root_view, cx.core.main_view].into_iter().flatten() {
            cx.with_any_view(view, |v, id, cx| v.window_closing(id, cx));
        }
        // A menu gone: its owner says what it shows again (it may no
        // longer be active: the menu was put away by a click elsewhere).
        if let (true, Some(owner)) = (core.menu, core.owner) {
            menu_gone(looper, owner);
        }
        core.end(looper);
    } else if let Some(windows) = looper.state_mut::<Windows>() {
        windows.map.insert(id, core);
    }
    Some(result)
}

/// A menu of window `owner`'s went: once what's running is done, the
/// views of it that show whether it's active say what they show again.
fn menu_gone(looper: &mut Looper, owner: WindowId) {
    looper.after(Duration::ZERO, move |looper| {
        with_window(looper, owner, |cx| {
            let by_it: Vec<ViewId> = cx.core.views.iter().filter(|(_, slot)| slot.reads_active).map(|(id, _)| id).collect();
            for view in by_it {
                cx.core.invalidate_view(view);
            }
        });
    });
}

/// When a window's drawings that move with time are drawn again
/// (`Painter::paint_again_within`): one clock for all of them, so they
/// move together. Each says, as it's drawn, how long until it would look
/// different (a graph: until its samples have moved a device pixel); the
/// clock ticks at the finest of those (a minute across 600 pixels is 10
/// frames a second, not 120 that would draw the same pixels), and each
/// tick draws all that asked. Graphs of many sizes ticking on their own
/// made a frame each (twelve small ones and a large one: twice the
/// frames). It's set going by the frame the drawings went in, so a
/// window that isn't shown (it gets no frames) has no clock; once
/// nothing asks, it stops.
pub(crate) struct Clock {
    /// The finest step asked before the last tick, and since.
    pace: f64,
    finest: f64,
    /// What asked since the last tick, and when the first of it did.
    nodes: Vec<NodeId>,
    asked_at: Option<Instant>,
    /// Waiting for the frame they were drawn in.
    waiting: bool,
    timer: Option<olo_app::TimerId>,
}

impl Clock {
    fn new() -> Clock {
        Clock { pace: f64::MAX, finest: f64::MAX, nodes: Vec::new(), asked_at: None, waiting: false, timer: None }
    }

    /// A node drawn just now is to be drawn again within `seconds`.
    pub(crate) fn ask(&mut self, node: NodeId, seconds: f64) {
        self.finest = self.finest.min(seconds);
        if !self.nodes.contains(&node) {
            self.nodes.push(node);
        }
        self.asked_at.get_or_insert_with(Instant::now);
    }
}

/// Does something with a window on a looper, by its number (from a
/// timer or a deferred turn, which have only the looper). None if it
/// has closed, or is the one that's running now.
pub fn with_window_on<R>(looper: &mut Looper, id: WindowId, f: impl FnOnce(&mut WindowCx) -> R) -> Option<R> {
    with_window(looper, id, f)
}

/// Hands a window what the display thread sent it.
pub(crate) fn deliver(looper: &mut Looper, id: WindowId, event: WindowEvent) {
    with_window(looper, id, |cx| cx.handle(event));
}

/// Has `f` run on a view (a message for it, most often).
pub(crate) fn deliver_to_view<V: View>(looper: &mut Looper, window: WindowId, view: ViewId, f: impl FnOnce(&mut V, &mut Cx<V>)) {
    with_window(looper, window, |cx| {
        cx.update_view(view, f);
    });
}

/// [`deliver_to_view`], for a view that may not be a `V` (nothing,
/// then).
pub(crate) fn deliver_to_view_if<V: View>(looper: &mut Looper, window: WindowId, view: ViewId, f: impl FnOnce(&mut V, &mut Cx<V>)) {
    with_window(looper, window, |cx| {
        if cx.read_view::<V>(view).is_some() {
            cx.update_view(view, f);
        }
    });
}

/// Opens a window on this looper, around the view `build` makes.
pub(crate) fn open<V: View>(looper: &mut Looper, id: WindowId, params: WindowParams, title: SharedString, build: impl FnOnce(&mut Cx<V>) -> V) -> anyhow::Result<WindowId> {
    let display = display();
    let link = WindowLink { looper: looper.handle(), id };
    let shell = params.layer_shell.is_some() || matches!(params.kind, crate::WindowKind::LayerShell | crate::WindowKind::SessionLock);
    let opened = display.open_window(id, params, link)?;
    let mut core = Box::new(WindowCore::new(id, opened, title));
    core.shell = shell;
    {
        let mut cx = WindowCx { core: &mut core, looper };
        let root = cx.new_view(build);
        let view = root.id();
        // (The window owns its own view: it ends with the window.)
        std::mem::forget(root);
        cx.core.root_view = Some(view);
        let node = cx.core.create(crate::Element::new(crate::element::Kind::View(view)), view, None);
        cx.core.root = Some(node);
    }
    if looper.state::<Windows>().is_none() {
        looper.set_state(Windows::default());
    }
    let windows = looper.state_mut::<Windows>().expect("set above");
    windows.map.insert(id, core);
    if !windows.hooked {
        windows.hooked = true;
        looper.on_idle(flush_all);
    }
    crate::app::window_opened();
    Ok(id)
}

/// Does something with every window on a looper.
pub(crate) fn each(looper: &mut Looper, mut f: impl FnMut(&mut WindowCx)) {
    let ids: Vec<WindowId> = looper.state::<Windows>().map(|w| w.map.keys().copied().collect()).unwrap_or_default();
    for id in ids {
        with_window(looper, id, &mut f);
    }
}

/// How many windows are on a looper.
pub(crate) fn count(looper: &Looper) -> usize {
    looper.state::<Windows>().map_or(0, |w| w.map.len())
}

/// How long what's invalidated for the next frame is kept when no frame
/// is asked for (the window isn't shown).
const HELD_AT_MOST: Duration = Duration::from_secs(1);

/// Every window on the looper shows what changed (the looper's queue is
/// dry).
fn flush_all(looper: &mut Looper) {
    // Views let go of by their owners end first.
    for (window, view) in crate::view::take_dropped() {
        with_window(looper, window, |cx| cx.core.end_view(view));
    }
    let ids: Vec<WindowId> = looper.state::<Windows>().map(|w| w.map.keys().copied().collect()).unwrap_or_default();
    for id in ids {
        with_window(looper, id, |cx| cx.flush());
    }
}

impl WindowCore {
    fn new(id: WindowId, opened: Opened, title: SharedString) -> WindowCore {
        let text_system = thread_text_system();
        let gfx = Gfx {
            scale_factor: opened.scale,
            rem_size: px(16.),
            text_rendering: opened.text_rendering,
            text_system,
            sprite_atlas: opened.atlas,
            svg_renderer: SvgRenderer::new(Arc::new(())),
            glyph_memo: FxHashMap::default(),
            outlines: Default::default(),
            text_style: TextStyle::default(),
            ext: None,
        };
        WindowCore {
            id,
            nodes: SlotMap::with_key(),
            views: SlotMap::with_key(),
            layout: Layout::new(),
            root: None,
            root_view: None,
            main_view: None,
            commits: 0,
            owner: None,
            shell: false,
            menu: false,
            gfx,
            size: opened.size,
            title,
            surface_id: opened.surface_id,
            display: opened.display,
            dirty_views: Vec::new(),
            paint_queue: Vec::new(),
            remeasured: Vec::new(),
            needs_commit: true,
            items: Vec::new(),
            items_stale: true,
            layers: Vec::new(),
            wheel: Vec::new(),
            scrollers: FxHashMap::default(),
            watchers: Vec::new(),
            scroll_changed: Vec::new(),
            scroll_asks: Vec::new(),
            hover_stale: false,
            within_stale: false,
            ended_timers: Vec::new(),
            by_visible: Vec::new(),
            with_externals: Vec::new(),
            row_nodes: Vec::new(),
            bounds_changed: Vec::new(),
            commit_held: None,
            state: WindowState::default(),
            relayout: true,
            everything: true,
            started: Some(Instant::now()),
            mouse: Point::default(),
            pointer_in: false,
            modifiers: Modifiers::default(),
            hovered: Vec::new(),
            dropping: None,
            drop_offer: None,
            drop_accept_sent: None,
            pressed: Vec::new(),
            focus: None,
            pending_focus: None,
            pending_focus_handle: None,
            fallback_focus: None,
            window_listeners: Vec::new(),
            active: false,
            closed: false,
            propagate: true,
            default_prevented: false,
            current: None,
            next_frame: Vec::new(),
            at_frame: Vec::new(),
            held_since: None,
            pace: opened.pace,
            clock: Clock::new(),
            cursor: crate::CursorStyle::Arrow,
            recycled: Vec::new(),
            kept: HashMap::new(),
            keyed: HashMap::new(),
        }
    }

    pub(crate) fn invalidate_view(&mut self, view: ViewId) {
        if let Some(slot) = self.views.get_mut(view)
            && !slot.dirty
        {
            slot.dirty = true;
            self.dirty_views.push(view);
        }
    }

    /// What `view` shows changed, and is wanted in the next frame drawn,
    /// no sooner (`Cx::invalidate_at_frame`).
    pub(crate) fn invalidate_view_at_frame(&mut self, view: ViewId) {
        if self.views.get(view).is_some_and(|slot| !slot.dirty) && !self.at_frame.contains(&view) {
            self.at_frame.push(view);
        }
    }

    /// What was kept for the next frame is laid out and drawn now, if a
    /// frame can be drawn now (`due`: the stage says so), or it has been
    /// kept long enough; else the stage is told there is something.
    fn release_at_frame(&mut self, due: bool) {
        use std::sync::atomic::Ordering::SeqCst;
        if self.at_frame.is_empty() {
            return;
        }
        let pace = &self.pace;
        let now = due
            || pace.unpaced
            // (Something else is laid out now: what's kept goes with
            // it, not a frame behind it.)
            || !self.dirty_views.is_empty()
            || !pace.waiting.load(SeqCst)
            // (A window that isn't shown is never asked for a frame, and
            // what watches it, an agent reading it, isn't left with what
            // it showed then for ever.)
            || self.held_since.is_some_and(|since| since.elapsed() >= HELD_AT_MOST)
            || {
                pace.held.store(true, SeqCst);
                // (The stage began a frame just then, and never saw
                // that: taken back, unless it did after all and asks.)
                !pace.waiting.load(SeqCst) && pace.held.swap(false, SeqCst)
            };
        if now {
            self.held_since = None;
            for view in std::mem::take(&mut self.at_frame) {
                self.invalidate_view(view);
            }
        } else {
            self.held_since.get_or_insert_with(Instant::now);
        }
    }

    /// A view's owner let go of it: it and what it showed are gone.
    fn end_view(&mut self, view: ViewId) {
        let Some(slot) = self.views.remove(view) else { return };
        self.ended_timers.extend(slot.timers.iter().copied());
        if let Some(root) = slot.root
            && let Some(node) = self.nodes.get_mut(root)
        {
            node.view_root = None;
            let parent = node.parent;
            if let Some(parent) = parent
                && let Some(p) = self.nodes.get_mut(parent)
            {
                p.children.retain(|c| *c != root);
                let layouts: Vec<_> = p.children.clone().iter().map(|c| self.nodes[*c].layout).collect();
                let layout = self.nodes[parent].layout;
                self.layout.set_children(layout, &layouts);
            }
            self.destroy(root);
        }
        // (Dropping the view drops the children it owned: they end at
        // the next flush.)
        drop(slot);
    }

    /// The first node under `id` (or it) that can have the keyboard.
    fn first_focusable(&self, id: NodeId) -> Option<NodeId> {
        let node = self.nodes.get(id)?;
        if node.inter.as_ref().is_some_and(|i| i.focusable && !i.disabled) {
            return Some(id);
        }
        node.children.iter().find_map(|child| self.first_focusable(*child))
    }

    /// Whether a node is in what the window shows (not in a view that
    /// isn't placed anywhere now).
    pub(crate) fn is_attached(&self, id: NodeId) -> bool {
        let mut at = Some(id);
        while let Some(id) = at {
            if Some(id) == self.root {
                return true;
            }
            at = self.nodes.get(id).and_then(|n| n.parent);
        }
        false
    }

    /// The tree as layout left it, to stderr (`OLO_DUMP_LAYOUT=1`): each
    /// node's kind, where it is in what holds it and in the window (in
    /// points), and the size and position its style asks for. For when
    /// something is in the wrong place and the agent tree doesn't say
    /// why.
    fn dump(&self, id: NodeId, depth: usize) {
        let node = &self.nodes[id];
        let (rel, _) = self.layout.placed(node.layout, self.gfx.scale_factor);
        let kind = match &node.kind {
            NodeKind::Group if node.view_root.is_some() => "view".to_owned(),
            NodeKind::Group => "group".to_owned(),
            NodeKind::Text(t) => format!("text {:?}", t.text.chars().take(24).collect::<String>()),
            NodeKind::Canvas(_) => "canvas".to_owned(),
        };
        let (at, style) = (node.bounds.origin, &node.style);
        eprintln!(
            "{:indent$}{kind}: {},{} {}x{}, at {},{}; asks {:?} x {:?}, {:?}{}",
            "",
            rel.origin.x.0,
            rel.origin.y.0,
            rel.size.width.0,
            rel.size.height.0,
            at.x.0,
            at.y.0,
            style.size.width,
            style.size.height,
            style.position,
            if node.scroll.is_some() { ", scrolls" } else { "" },
            indent = depth * 2
        );
        for child in &node.children {
            self.dump(*child, depth + 1);
        }
    }

    /// Works out where everything under `id` is, from what layout made
    /// of it. What it and all it holds reach.
    fn place(&mut self, id: NodeId, parent_origin: Point<Pixels>) -> Bounds<Pixels> {
        let scale = self.gfx.scale_factor;
        let Some(node) = self.nodes.get_mut(id) else { return Bounds::default() };
        let (rel, content) = self.layout.placed(node.layout, scale);
        let bounds = Bounds { origin: parent_origin + rel.origin, size: rel.size };
        let moved = bounds != node.bounds;
        let resized = bounds.size != node.bounds.size;
        node.bounds = bounds;
        node.content_size = content;
        // (Whatever draws, not only what has a drawing kept: one made
        // with no room, a field in a window not yet given its size, is
        // empty, and there's something to draw once there's room.)
        let mut repaint = resized && node.paints();
        if moved && !repaint {
            // What it drew is moved with it, if that's by whole device
            // pixels (it nearly always is: layout lands on them).
            let off = |a: Pixels, b: Pixels| {
                let d = (a.0 - b.0) * scale;
                (d - d.round()).abs() > 0.01
            };
            // A drawing of the app's own, by whole points too: what it
            // rounds to a point (a check box's square, its middle less
            // 6.5 points) comes out a pixel elsewhere drawn afresh half
            // a point along at 2×, and moved, it stayed a pixel off its
            // label until something drew it again.
            let off_points = |a: Pixels, b: Pixels| {
                let d = a.0 - b.0;
                (d - d.round()).abs() > 0.01
            };
            let drawn = matches!(node.kind, NodeKind::Canvas(_));
            repaint = off(bounds.origin.x, node.painted_at.x)
                || off(bounds.origin.y, node.painted_at.y)
                || (drawn && (off_points(bounds.origin.x, node.painted_at.x) || off_points(bounds.origin.y, node.painted_at.y)));
        }
        if moved {
            // (And the pointer may be over something else now.)
            self.reshaped();
            self.hover_stale = true;
            if self.nodes[id].inter.as_ref().is_some_and(|i| !i.bounds_listeners.is_empty()) {
                self.bounds_changed.push(id);
            }
        }
        if repaint {
            self.mark_paint(id);
        }
        let mut reach = bounds;
        let count = self.nodes[id].children.len();
        for i in 0..count {
            let child = self.nodes[id].children[i];
            let theirs = self.place(child, bounds.origin);
            if !theirs.is_empty() {
                reach = reach.union(&theirs);
            }
        }
        if self.nodes[id].scroll.is_some() {
            // (What it holds is under it, wherever it's scrolled to.)
            reach = bounds;
            self.measure_scroll(id);
        }
        self.nodes[id].reach = reach;
        reach
    }

    /// How far a scroller goes, from what it holds as just laid out
    /// (GPUI's sum: its children's extent and its padding, less its own
    /// size); kept within that if it shrank.
    fn measure_scroll(&mut self, id: NodeId) {
        let node = &self.nodes[id];
        let Some(handle) = node.scroll.clone() else { return };
        let bounds = node.bounds;
        let mut children = handle.0.children.borrow_mut();
        children.clear();
        let (mut low, mut high) = (point(Pixels::MAX, Pixels::MAX), Point::default());
        for child in &node.children {
            let Some(child) = self.nodes.get(*child) else { continue };
            low = low.min(&child.bounds.origin);
            high = high.max(&child.bounds.bottom_right());
            children.push(Bounds { origin: child.bounds.origin - bounds.origin, size: child.bounds.size });
        }
        drop(children);
        let content: Size<Pixels> = if node.children.is_empty() { bounds.size } else { size(high.x - low.x, high.y - low.y) };
        let padding = node.style.padding.to_pixels(bounds.size.into(), self.gfx.rem_size);
        let round = |v: Pixels| px((v.0 * 100.0).round() / 100.0).max(px(0.));
        let mut max = size(round(content.width + padding.left + padding.right - bounds.size.width), round(content.height + padding.top + padding.bottom - bounds.size.height));
        if node.style.overflow.x != crate::Overflow::Scroll {
            max.width = px(0.);
        }
        if node.style.overflow.y != crate::Overflow::Scroll {
            max.height = px(0.);
        }
        let changed = handle.0.max.get() != max || handle.0.bounds.get() != bounds;
        handle.0.max.set(max);
        handle.0.bounds.set(bounds);
        let at = handle.offset();
        let within = point(at.x.clamp(-max.width, px(0.)), at.y.clamp(-max.height, px(0.)));
        if within != at {
            handle.put(within);
        }
        let ways = crate::scroll::moved(at, within) | if changed { crate::scroll::ENDS } else { 0 };
        if ways != 0 {
            self.scroll_moved(handle.id(), ways);
        }
    }

    /// A scroller moved, or its ends did: the stage is told, and what
    /// hears of it or draws by it does at the next flush.
    pub(crate) fn scroll_moved(&mut self, id: u64, ways: u8) {
        self.reshaped();
        self.hover_stale = true;
        self.note_scrolled(id, ways);
    }

    /// What hears of scroller `id` is to hear it changed (`ways`).
    pub(crate) fn note_scrolled(&mut self, id: u64, ways: u8) {
        match self.scroll_changed.iter_mut().find(|(changed, _)| *changed == id) {
            Some((_, was)) => *was |= ways,
            None => self.scroll_changed.push((id, ways)),
        }
    }

    /// A scroller was asked to go somewhere once it's laid out.
    pub(crate) fn scroll_asked(&mut self, handle: crate::ScrollHandle) {
        self.needs_commit = true;
        if !self.scroll_asks.contains(&handle) {
            self.scroll_asks.push(handle);
        }
    }

    /// Makes the list of everything that shows, in painting order, for
    /// the stage (and notes where each node's drawings are in it), with
    /// what scrolls: its layers, and where the wheel means something.
    fn list_items(&mut self) {
        let scale = self.gfx.scale_factor;
        let window = Bounds { origin: point(ScaledPixels(0.), ScaledPixels(0.)), size: size(ScaledPixels(self.size.width.0 * scale), ScaledPixels(self.size.height.0 * scale)) };
        let mut listing = Listing {
            items: std::mem::take(&mut self.items),
            layers: std::mem::take(&mut self.layers),
            wheel: std::mem::take(&mut self.wheel),
            scrollers: std::mem::take(&mut self.scrollers),
            watchers: std::mem::take(&mut self.watchers),
            raised: Some(Vec::new()),
        };
        listing.items.clear();
        listing.layers.clear();
        listing.wheel.clear();
        listing.scrollers.clear();
        listing.watchers.clear();
        listing.layers.push(Layer { id: 0, parent: 0, viewport: window, max: Size::default(), view: self.size, axes: (false, false), set: None });
        // (What the listing doesn't reach, held by something hidden or
        // cut away whole, is in no place in it: a place kept from the
        // list before was another node's in this one, and its next
        // drawing went there. Full screen, the Viewer's hidden menu bar
        // took its video's place.)
        for (_, node) in self.nodes.iter_mut() {
            node.slots = [UNLISTED; 2];
        }
        if let Some(root) = self.root {
            self.collect(root, window, 0, &mut listing);
            // What's raised, over the rest, cut by the window alone.
            for (id, layer) in listing.raised.take().unwrap_or_default() {
                self.collect(id, window, layer, &mut listing);
            }
        }
        let Listing { items, layers, wheel, scrollers, watchers, .. } = listing;
        (self.items, self.layers, self.wheel, self.scrollers, self.watchers) = (items, layers, wheel, scrollers, watchers);
        self.items_stale = false;
    }

    fn collect(&mut self, id: NodeId, clip: Bounds<ScaledPixels>, layer: u32, out: &mut Listing) {
        let scale = self.gfx.scale_factor;
        let rem = self.gfx.rem_size;
        let Some(node) = self.nodes.get_mut(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        node.slots = [u32::MAX; 2];
        if node.inter.as_ref().is_some_and(|i| i.raised)
            && let Some(raised) = out.raised.as_mut()
        {
            raised.push((id, layer));
            return;
        }
        if let Some(inter) = &node.inter {
            // What takes the wheel itself, or lets nothing under it
            // have the pointer: the stage leaves the wheel there alone.
            if !inter.scroll_wheel.is_empty() || inter.occlude {
                let rect = node.bounds.scale(scale).intersect(&clip);
                if !rect.is_empty() {
                    out.wheel.push(WheelRegion { layer, rect, kind: WheelKind::Taker });
                }
            }
            out.watchers.extend(inter.watches.iter().map(|(watched, ways)| (*watched, *ways, id)));
        }
        let by = point(ScaledPixels(((node.bounds.origin.x.0 - node.painted_at.x.0) * scale).round()), ScaledPixels(((node.bounds.origin.y.0 - node.painted_at.y.0) * scale).round()));
        let shows = |list: &Arc<crate::stage::Recording>| !Bounds { origin: list.bounds.origin + by, size: list.bounds.size }.intersect(&clip).is_empty();
        if let Some(list) = &node.behind
            && shows(list)
        {
            node.slots[0] = out.items.len() as u32;
            out.items.push(Item { list: list.clone(), by, clip, layer });
        }
        let count = node.children.len();
        if count > 0 {
            let inner = match node.style.overflow_mask(node.bounds, rem, scale) {
                Some(mask) => clip.intersect(&mask.scale(scale).bounds),
                None => clip,
            };
            // What scrolls is a layer of its own: everything in it is
            // listed where it's laid out, all of it, and the stage shows
            // it from wherever it's scrolled to.
            let (inner, within) = match &node.scroll {
                Some(handle) if !inner.is_empty() => {
                    let index = out.layers.len() as u32;
                    let scrolls = (node.style.overflow.x == crate::Overflow::Scroll, node.style.overflow.y == crate::Overflow::Scroll);
                    let set = handle.0.set.take().then(|| (handle.offset(), handle.0.epoch.get()));
                    out.layers.push(Layer { id: handle.id(), parent: layer, viewport: inner, max: handle.max_offset(), view: node.bounds.size, axes: scrolls, set });
                    out.wheel.push(WheelRegion { layer, rect: inner, kind: WheelKind::Scroller(index) });
                    out.scrollers.insert(handle.id(), id);
                    (EVERYWHERE, index)
                }
                _ => (inner, layer),
            };
            if !inner.is_empty() {
                for i in 0..count {
                    let child = self.nodes[id].children[i];
                    self.collect(child, inner, within, out);
                }
            }
        }
        let node = &mut self.nodes[id];
        if let Some(list) = &node.over
            && shows(list)
        {
            node.slots[1] = out.items.len() as u32;
            out.items.push(Item { list: list.clone(), by, clip, layer });
        }
    }

    /// A node drew again and nothing else about it changed: its place in
    /// the list of what shows takes the new drawing. False if the list
    /// has to be made again (it now draws something where it drew
    /// nothing, or the other way).
    fn patch_items(&mut self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(id) else { return true };
        // (Not reached by the list: whatever it draws, it doesn't show.)
        if node.slots == [UNLISTED; 2] {
            return true;
        }
        for (slot, list) in [(node.slots[0], &node.behind), (node.slots[1], &node.over)] {
            match (self.items.get_mut(slot as usize), list) {
                (Some(item), Some(list)) => {
                    item.list = list.clone();
                    item.by = Point::default();
                }
                (None, None) => {}
                // (Drawn where nothing was, or nothing where something
                // was; or it was clipped away whole before.)
                _ => return false,
            }
        }
        true
    }

    /// What agents read under a node: `shift` is how far it shows from
    /// where it's laid out (what holds it scrolled), `clip` what can be
    /// seen there, in the window.
    fn agent_nodes(&self, id: NodeId, clip: Bounds<Pixels>, shift: Point<Pixels>, out: &mut Vec<crate::agent::Node>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        let shown = |b: Bounds<Pixels>| Bounds { origin: b.origin + shift, size: b.size };
        // What it is itself, if it says.
        if let Some(inter) = &node.inter
            && let Some(agent) = &inter.agent
        {
            out.push(crate::agent::Node {
                role: agent.role,
                name: agent.name.clone(),
                value: agent.value.clone(),
                bounds: shown(node.bounds),
                enabled: !inter.disabled,
                clickable: !inter.disabled && agent.clickable.unwrap_or(!inter.click.is_empty() || !inter.mouse_down.is_empty()),
                focused: node.focused,
                checked: agent.checked,
                selected: agent.selected,
                minimized: false,
                clip: Some(clip),
            });
        }
        // What its drawing said it holds, moved as the drawing is.
        if let Some(notes) = &node.notes {
            let moved = point(node.bounds.origin.x - node.painted_at.x, node.bounds.origin.y - node.painted_at.y) + shift;
            for noted in &notes.agent {
                let mut noted = noted.clone();
                noted.bounds.origin = noted.bounds.origin + moved;
                noted.clip = Some(noted.clip.map_or(clip, |c| Bounds { origin: c.origin + moved, size: c.size }.intersect(&clip)));
                out.push(noted);
            }
        }
        let inner = match node.style.overflow_mask(node.bounds, self.gfx.rem_size, self.gfx.scale_factor) {
            Some(mask) => clip.intersect(&shown(mask.bounds)),
            None => clip,
        };
        let shift = match &node.scroll {
            Some(handle) => shift + handle.0.shown.get(),
            None => shift,
        };
        for child in &node.children {
            self.agent_nodes(*child, inner, shift, out);
        }
    }

    /// The texts that don't show in full under a node, where what
    /// shows of each is in the window: `clip` is what can be seen there.
    fn cut_texts(&self, id: NodeId, clip: Bounds<Pixels>, shift: Point<Pixels>, out: &mut Vec<(Bounds<Pixels>, SharedString)>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        if let Some(notes) = &node.notes {
            let moved = point(node.bounds.origin.x - node.painted_at.x, node.bounds.origin.y - node.painted_at.y) + shift;
            for (b, full, if_cut) in &notes.cuts {
                let b = Bounds { origin: b.origin + moved, size: b.size };
                let seen = b.intersect(&clip);
                if seen.size.width <= px(0.) || seen.size.height <= px(0.) {
                    continue;
                }
                // (A pixel's grace: a label flush against its clip
                // isn't cut.)
                if *if_cut && !(b.left() < clip.left() - px(1.0) || b.right() > clip.right() + px(1.0)) {
                    continue;
                }
                out.push((seen, full.clone()));
            }
        }
        let shown = |b: Bounds<Pixels>| Bounds { origin: b.origin + shift, size: b.size };
        let inner = match node.style.overflow_mask(node.bounds, self.gfx.rem_size, self.gfx.scale_factor) {
            Some(mask) => clip.intersect(&shown(mask.bounds)),
            None => clip,
        };
        let shift = match &node.scroll {
            Some(handle) => shift + handle.0.shown.get(),
            None => shift,
        };
        for child in &node.children {
            self.cut_texts(*child, inner, shift, out);
        }
    }

    /// The window is gone: what it held goes, and the program hears.
    fn end(mut self: Box<Self>, looper: &mut Looper) {
        let id = self.id;
        // What belonged to it (its menus, its tips) goes with it.
        if let Some(windows) = looper.state_mut::<Windows>() {
            for core in windows.map.values_mut().filter(|core| core.owner == Some(id)) {
                core.closed = true;
            }
        }
        display().post(move |on| {
            if let Some(stage) = on.stages.remove(&id) {
                stage.close();
            }
        });
        // Its views' timers stop with it (on a looper it shares with
        // the window it belongs to, nothing else would stop them).
        for timer in self.views.values().flat_map(|slot| slot.timers.iter().copied()).chain(self.ended_timers.drain(..)).collect::<Vec<_>>() {
            looper.cancel(timer);
        }
        // (Views first: what they own may want the tree.)
        self.views.clear();
        self.nodes.clear();
        crate::app::window_closed();
    }
}

impl<'a> WindowCx<'a> {
    /// Takes what the display thread sent.
    fn handle(&mut self, event: WindowEvent) {
        self.core.started.get_or_insert_with(Instant::now);
        match event {
            WindowEvent::Input(input) => self.dispatch_input(input),
            WindowEvent::Ime(ime) => self.dispatch_ime(ime),
            WindowEvent::Resized { size, scale } => self.resized(size, scale),
            WindowEvent::Active(active) => {
                self.core.active = active;
                let by_it: Vec<ViewId> = self.core.views.iter().filter(|(_, slot)| slot.reads_active).map(|(id, _)| id).collect();
                for view in by_it {
                    self.core.invalidate_view(view);
                }
                if let Some(view) = self.core.main_view.or(self.core.root_view) {
                    self.with_any_view(view, |view, id, cx| view.window_activated(id, active, cx));
                }
            }
            WindowEvent::Hovered(hovered) => {
                if !hovered {
                    self.pointer_left();
                }
            }
            WindowEvent::Moved => {}
            WindowEvent::Display(display) => self.core.display = display,
            WindowEvent::CloseRequested => {
                let may = match self.core.main_view.or(self.core.root_view) {
                    Some(view) => self.with_any_view(view, |v, id, cx| v.close_requested(id, cx)).unwrap_or(true),
                    None => true,
                };
                if may {
                    self.close_window();
                }
            }
            WindowEvent::Closed => self.core.closed = true,
            WindowEvent::TextRendering(rendering) => {
                if self.core.gfx.text_rendering != rendering {
                    self.core.gfx.text_rendering = rendering;
                    self.refresh();
                }
            }
            WindowEvent::Frame => {
                for f in std::mem::take(&mut self.core.next_frame) {
                    f(self);
                }
                // (What's kept for a frame goes with what animates.)
                self.core.release_at_frame(true);
            }
            // (Drawn as the queue runs dry: `flush`.)
            WindowEvent::FrameDue => self.core.release_at_frame(true),
            WindowEvent::Scrolled(report) => self.scrolled(report),
            WindowEvent::State(state) => {
                if self.core.state != state {
                    self.core.state = state;
                    // (The window's first view lays out by it, and so
                    // does whatever asked how the window is held or how
                    // big it is: a video's transport full screen.)
                    let by_it: Vec<ViewId> = self.core.views.iter().filter(|(_, slot)| slot.reads_size).map(|(id, _)| id).chain(self.core.root_view).collect();
                    for view in by_it {
                        self.core.invalidate_view(view);
                    }
                }
            }
        }
    }

    /// What hears where layout put it hears.
    fn tell_bounds(&mut self) {
        for id in std::mem::take(&mut self.core.bounds_changed) {
            let Some(listeners) = self.core.nodes.get(id).and_then(|n| n.inter.as_ref()).map(|i| i.bounds_listeners.clone()) else { continue };
            let bounds = self.core.window_bounds(id);
            self.core.current = Some(id);
            for listener in listeners {
                listener(&bounds, self);
            }
            self.core.current = None;
        }
    }

    /// The stage says where the scrollers are (the wheel or the
    /// touchpad moved one, or an end let go).
    fn scrolled(&mut self, report: Vec<ScrollReport>) {
        let mut any = false;
        for (id, offset, shown, epoch) in report {
            let Some(handle) = self.core.scrollers.get(&id).and_then(|node| self.core.nodes.get(*node)).and_then(|n| n.scroll.clone()) else { continue };
            // (From before the looper last moved it itself: stale.)
            if epoch != handle.0.epoch.get() {
                continue;
            }
            any |= handle.0.shown.replace(shown) != shown;
            let ways = crate::scroll::moved(handle.0.offset.replace(offset), offset);
            if ways != 0 {
                any = true;
                self.core.note_scrolled(id, ways);
            }
        }
        if any {
            self.core.hover_stale = true;
            self.tell_scrolled();
            self.core.update_rows();
            self.core.check_visible();
            if std::mem::take(&mut self.core.hover_stale) {
                self.update_hover();
            }
        }
    }

    /// What hears of a scroller that moved hears, and what's drawn by it
    /// is drawn again.
    fn tell_scrolled(&mut self) {
        while !self.core.scroll_changed.is_empty() {
            for (id, ways) in std::mem::take(&mut self.core.scroll_changed) {
                let watching: Vec<NodeId> = self.core.watchers.iter().filter(|(watched, which, _)| *watched == id && which & ways != 0).map(|(_, _, node)| *node).collect();
                for node in watching {
                    self.core.mark_paint(node);
                    // What hears of a scroller it doesn't hold.
                    let hearing: Vec<_> =
                        self.core.nodes.get(node).and_then(|n| n.inter.as_ref()).map(|i| i.scroll_of_listeners.iter().filter(|(h, _)| h.id() == id).cloned().collect()).unwrap_or_default();
                    self.core.current = Some(node);
                    for (handle, listener) in hearing {
                        listener(&handle.offset(), self);
                    }
                    self.core.current = None;
                }
                // (A scroller laid out for the first time this turn isn't
                // listed yet, and its ends just came to be: found by its
                // handle, so what hears of it does. A bar that shows only
                // while there is something to scroll came, else, only
                // once its view was scrolled.)
                let is = |n: &Node| n.scroll.as_ref().is_some_and(|h| h.id() == id);
                let listed = self.core.scrollers.get(&id).copied().filter(|node| self.core.nodes.get(*node).is_some_and(is));
                let Some(node) = listed.or_else(|| self.core.nodes.iter().find(|(_, n)| is(n)).map(|(node, _)| node)) else { continue };
                let Some((offset, listeners)) = self.core.nodes.get(node).and_then(|n| Some((n.scroll.as_ref()?.offset(), n.inter.as_ref()?.scroll_listeners.clone()))) else { continue };
                self.core.current = Some(node);
                for listener in listeners {
                    listener(&offset, self);
                }
                self.core.current = None;
            }
        }
    }

    fn resized(&mut self, size: Size<Pixels>, scale: f32) {
        let core = &mut *self.core;
        if core.gfx.scale_factor != scale {
            core.gfx.scale_factor = scale;
            core.gfx.glyph_memo.clear();
            // Every length is in device pixels to layout: all of it is
            // said again at the new scale, and drawn again.
            let rem = core.gfx.rem_size;
            for (_, node) in core.nodes.iter_mut() {
                // (What's scrolled stays on device pixels: an offset that
                // was on the old scale's grid is a fraction of a pixel
                // on the new one's, and every row with it.)
                if let Some(handle) = &node.scroll {
                    handle.snap(scale);
                }
                core.layout.set_style(node.layout, &node.style, rem, scale);
                if let NodeKind::Text(text) = &mut node.kind {
                    text.laid = None;
                    core.layout.mark_dirty(node.layout);
                }
            }
            self.refresh();
        }
        let core = &mut *self.core;
        if core.size != size {
            core.size = size;
            SIZE.with(|s| s.set(size));
            core.relayout = true;
            core.reshaped();
            // (Views that show by the window's size say again.)
            let sized: Vec<ViewId> = core.views.iter().filter(|(_, slot)| slot.reads_size).map(|(id, _)| id).collect();
            for view in sized {
                core.invalidate_view(view);
            }
        }
    }

    /// The frame that what moves with time was drawn in is showing: the
    /// clock is set for its next tick (what's left of the finest step
    /// asked, a 240th of a second to a minute).
    fn wind_clock(&mut self) {
        let clock = &mut self.core.clock;
        clock.waiting = false;
        if clock.nodes.is_empty() || clock.timer.is_some() {
            return;
        }
        let step = clock.pace.min(clock.finest).clamp(1.0 / 240.0, 60.0);
        let since = clock.asked_at.map_or(0.0, |at| at.elapsed().as_secs_f64());
        if step <= since {
            // (Due already: asked for every frame, a screen saver's
            // drawing, it's drawn for the next as this one shows, with
            // no timer between.)
            return self.tick_clock();
        }
        let id = self.core.id;
        let timer = self.looper.after(Duration::from_secs_f64((step - since).max(0.0)), move |looper| {
            with_window(looper, id, |cx| cx.tick_clock());
        });
        self.core.clock.timer = Some(timer);
    }

    /// The clock ticks: all that asked is drawn again (and asks again as
    /// it is, if it still moves).
    fn tick_clock(&mut self) {
        let clock = &mut self.core.clock;
        clock.timer = None;
        clock.asked_at = None;
        clock.pace = std::mem::replace(&mut clock.finest, f64::MAX);
        for id in std::mem::take(&mut clock.nodes) {
            if self.core.nodes.contains_key(id) {
                self.core.mark_paint(id);
            }
        }
    }

    /// What was drawn while something it shows was still coming (a
    /// picture being decoded: `Painter::paint_unfinished`) is drawn
    /// again: it has come.
    pub fn repaint_unfinished(&mut self) {
        let waiting: Vec<NodeId> = self.core.nodes.iter().filter(|(_, n)| n.notes.as_ref().is_some_and(|notes| notes.unfinished)).map(|(id, _)| id).collect();
        for id in waiting {
            self.core.mark_paint(id);
        }
    }

    /// Everything is drawn again (the look changed: the theme, the
    /// scale, how text is drawn). Not for one thing changing: that's
    /// its view's [`Cx::invalidate`].
    pub fn refresh(&mut self) {
        let core = &mut *self.core;
        core.gfx.glyph_memo.clear();
        core.everything = true;
        core.reshaped();
        let views: Vec<ViewId> = core.views.keys().collect();
        for view in views {
            core.invalidate_view(view);
        }
        if let Some(root) = core.root {
            core.mark_subtree(root, true);
        }
    }

    /// Shows what changed: views say what they show, the tree follows,
    /// layout and drawing catch up, and the stage is told.
    fn flush(&mut self) {
        if self.core.closed {
            return;
        }
        // What's kept for the next frame, if one can be drawn now.
        self.core.release_at_frame(false);
        let flushing = Instant::now();
        let phases = crate::stats::frame_phases();
        let mut turns = 0;
        let mut rounds = 0;
        let mut bodies_done = Duration::ZERO;
        loop {
            rounds += 1;
            // What hears of scrolling hears (it may have more to show).
            self.tell_scrolled();
            if std::mem::take(&mut self.core.hover_stale) {
                self.update_hover();
            }
            // What changed says what it shows now.
            let bodies = Instant::now();
            while let Some(view) = self.core.dirty_views.pop() {
                turns += 1;
                if turns > 100_000 {
                    tracing::error!("a view invalidates itself in its own body, for ever");
                    break;
                }
                let Some(slot) = self.core.views.get_mut(view) else { continue };
                slot.dirty = false;
                let Some(root) = slot.root else { continue };
                if !self.core.is_attached(root) {
                    // (Not shown now: it says what it shows when it's
                    // placed again, `place_view`.)
                    continue;
                }
                let Some(mut boxed) = self.core.views[view].view.take() else { continue };
                SIZE_READ.with(|r| r.set(false));
                ACTIVE_READ.with(|r| r.set(false));
                let mut element = boxed.body(view, self);
                if let Some(slot) = self.core.views.get_mut(view) {
                    slot.view = Some(boxed);
                    slot.reads_size = SIZE_READ.with(|r| r.get());
                    slot.reads_active = ACTIVE_READ.with(|r| r.get());
                }
                if matches!(element.kind, crate::element::Kind::View(_)) {
                    // (A view that is only another view: held in a box
                    // of its own, as the node is this view's.)
                    use crate::{ParentElement as _, Styled as _};
                    element = crate::div().size_full().child(element);
                }
                if self.core.nodes.contains_key(root) {
                    self.core.reconcile(root, element, view);
                }
            }
            // The keyboard to a view that asked for it before it was
            // shown.
            if let Some(view) = self.core.pending_focus.take() {
                self.focus_view(view);
            }
            // What was made anew round what has the keyboard has it
            // within it.
            if std::mem::take(&mut self.core.within_stale)
                && let Some(focus) = self.core.focus
            {
                let mut at = self.core.nodes.get(focus).and_then(|n| n.parent);
                while let Some(id) = at {
                    let Some(node) = self.core.nodes.get_mut(id) else { break };
                    at = node.parent;
                    if node.inter.as_ref().is_some_and(|i| i.focus_within) && !node.focused {
                        node.focused = true;
                        self.core.state_changed(id);
                    }
                }
            }
            // (Asked for before what takes it was there, a view's element
            // not yet made by its first `body`: kept until it is. Taken
            // and dropped when it wasn't there yet, Rescue's terminal had
            // the keyboard on some starts and not on others.)
            if let Some(handle) = self.core.pending_focus_handle
                && let Some(node) =
                    self.core.nodes.iter().find(|(id, n)| n.inter.as_ref().is_some_and(|i| i.focus_handle == Some(handle) && !i.disabled) && self.core.is_attached(*id)).map(|(id, _)| id)
            {
                self.core.pending_focus_handle = None;
                self.focus_node(Some(node));
            }
            // (Nothing has the keyboard: what takes it then does.)
            if self.core.focus.is_none()
                && self.core.pending_focus.is_none()
                && let Some(handle) = self.core.fallback_focus
                && let Some(node) =
                    self.core.nodes.iter().find(|(id, n)| n.inter.as_ref().is_some_and(|i| i.focus_handle == Some(handle) && !i.disabled) && self.core.is_attached(*id)).map(|(id, _)| id)
            {
                self.focus_node(Some(node));
            }
            bodies_done += bodies.elapsed();
            let core = &mut *self.core;
            // Where everything goes.
            if let Some(root) = core.root {
                let root_layout = core.nodes[root].layout;
                if core.relayout || core.layout.is_dirty(root_layout) {
                    core.relayout = false;
                    let available = size(AvailableSpace::Definite(core.size.width), AvailableSpace::Definite(core.size.height));
                    let scale = core.gfx.scale_factor;
                    let mut layout = std::mem::replace(&mut core.layout, Layout::new());
                    layout.compute(root_layout, available, scale, |node, known, available| core.measure(node, known, available));
                    core.layout = layout;
                    core.place(root, Point::default());
                    if crate::stats::dump_layout() {
                        eprintln!("layout at {:?}", core.size);
                        core.dump(root, 1);
                    }
                    for id in std::mem::take(&mut core.remeasured) {
                        core.mark_paint(id);
                    }
                }
            }
            // What hears where layout put it hears (and may have more
            // to say: round again).
            let told = !core.bounds_changed.is_empty();
            if told {
                self.tell_bounds();
            }
            let core = &mut *self.core;
            // What was asked to scroll somewhere once it was laid out.
            for handle in std::mem::take(&mut core.scroll_asks) {
                let ways = handle.settle(core.gfx.scale_factor);
                if ways != 0 {
                    core.scroll_moved(handle.id(), ways);
                }
            }
            // Rows that came near what shows are made (and laid out,
            // the next time round).
            let rows = core.update_rows();
            // (Scrolling that something heard of may have changed what a
            // view shows: round again, until it's all said.)
            let layout_stale = core.relayout || core.root.is_some_and(|root| core.layout.is_dirty(core.nodes[root].layout));
            if (core.scroll_changed.is_empty() && core.dirty_views.is_empty() && !core.hover_stale && !rows && !(told && layout_stale)) || rounds >= 8 {
                break;
            }
        }
        let core = &mut *self.core;
        core.check_visible();
        let layout_done = flushing.elapsed();
        let bodies_done = bodies_done.min(layout_done);
        // What changed is drawn.
        let mut drew = 0;
        let mut recorded: Vec<NodeId> = Vec::new();
        while !core.paint_queue.is_empty() {
            for id in std::mem::take(&mut core.paint_queue) {
                if core.nodes.get(id).is_some_and(|n| n.paint_dirty) {
                    if core.is_attached(id) {
                        core.record(id);
                        recorded.push(id);
                        drew += 1;
                    } else {
                        // (Drawn when it's shown again.)
                        core.nodes[id].paint_dirty = false;
                        core.nodes[id].behind = None;
                        core.nodes[id].over = None;
                    }
                }
            }
        }
        if drew > 0 {
            core.gfx.text_system.finish_frame();
        }
        // What moves with time is drawn again in a while, counted from
        // the frame this drawing goes in.
        if !core.clock.nodes.is_empty() && core.clock.timer.is_none() && !core.clock.waiting {
            core.clock.waiting = true;
            core.next_frame.push(Box::new(|cx| cx.wind_clock()));
            core.needs_commit = true;
        }
        let paint_done = flushing.elapsed();
        if !core.needs_commit {
            core.started = None;
            return;
        }
        // Held back (a window being sized to fit what it shows isn't
        // shown at the size it's leaving): committed when it's let go,
        // or the wait is over.
        if let Some(until) = core.commit_held {
            if Instant::now() < until {
                return;
            }
            core.commit_held = None;
        }
        core.needs_commit = false;
        if !core.items_stale {
            // Only drawings changed: each takes its place in the list.
            for id in &recorded {
                if !core.patch_items(*id) {
                    core.items_stale = true;
                    break;
                }
            }
        }
        if core.items_stale {
            core.list_items();
        }
        let items = core.items.clone();
        if phases {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let work = crate::stats::FRAME_WORK.take();
            eprintln!(
                "flush {:.3} ms: bodies {:.3} ({turns}), layout {:.3}, paint {:.3} ({drew} of {} nodes), items {:.3} ({}); shaped {} lines in {:.2} ms, drew {} glyphs in {:.2} ms",
                ms(flushing.elapsed()),
                ms(bodies_done),
                ms(layout_done - bodies_done),
                ms(paint_done - layout_done),
                core.nodes.len(),
                ms(flushing.elapsed() - paint_done),
                items.len(),
                work[0].0,
                work[0].1,
                work[1].0,
                work[1].1,
            );
        }
        let scale = core.gfx.scale_factor;
        let (layers, wheel) = (core.layers.clone(), core.wheel.clone());
        // (Told once where the looper put a scroller: after that the
        // stage's own word stands.)
        for layer in &mut core.layers {
            layer.set = None;
        }
        core.commits += 1;
        let started = core.started.take().unwrap_or(flushing);
        // Where other programs' pictures show now: where each was drawn,
        // moved as its node has been since (laid out elsewhere, scrolled).
        let mut externals = Vec::new();
        if !core.with_externals.is_empty() {
            let nodes = std::mem::take(&mut core.with_externals);
            for id in nodes {
                let Some(node) = core.nodes.get(id) else { continue };
                let Some(notes) = node.notes.as_ref().filter(|n| !n.externals.is_empty()) else { continue };
                let shift = core.shift_at(id);
                let moved = crate::point(node.bounds.origin.x - node.painted_at.x + shift.x, node.bounds.origin.y - node.painted_at.y + shift.y);
                externals.extend(notes.externals.iter().map(|(key, b)| (*key, Bounds { origin: crate::point(b.origin.x + moved.x, b.origin.y + moved.y), size: b.size })));
                core.with_externals.push(id);
            }
        }
        let commit =
            Commit { size: core.size, scale, items, layers, wheel, work: started.elapsed(), everything: std::mem::take(&mut core.everything), animating: !core.next_frame.is_empty(), externals };
        let id = core.id;
        // (On its way to be drawn: what's invalidated for a frame from
        // here on is for the one after it.)
        core.pace.waiting.store(true, std::sync::atomic::Ordering::SeqCst);
        display().post(move |on| {
            if let Some(stage) = on.stage(id) {
                stage.commit(commit);
            }
        });
    }

    // ---- Views ----

    /// Makes a view in this window; whoever keeps the [`Child`] owns it.
    pub fn new_view<T: View>(&mut self, build: impl FnOnce(&mut Cx<T>) -> T) -> Child<T> {
        let id = self.core.views.insert(ViewSlot { view: None, root: None, dirty: false, reads_size: false, reads_active: false, subscribers: Vec::new(), timers: Vec::new() });
        let view = build(&mut Cx::new(self, id));
        if let Some(slot) = self.core.views.get_mut(id) {
            slot.view = Some(Box::new(view));
        }
        Child::new(id, self.core.id)
    }

    /// Reaches into a view. None if it's gone, or is the one that's
    /// running now.
    pub(crate) fn update_view<V: View, R>(&mut self, id: ViewId, f: impl FnOnce(&mut V, &mut Cx<V>) -> R) -> Option<R> {
        let mut boxed = self.core.views.get_mut(id)?.view.take()?;
        let result = boxed.as_any_mut().downcast_mut::<V>().map(|view| f(view, &mut Cx::new(self, id)));
        debug_assert!(result.is_some(), "a view taken for another type");
        if let Some(slot) = self.core.views.get_mut(id) {
            slot.view = Some(boxed);
        }
        result
    }

    /// Reaches into a view of whatever type it is.
    pub(crate) fn with_any_view<R>(&mut self, id: ViewId, f: impl FnOnce(&mut dyn AnyView, ViewId, &mut WindowCx) -> R) -> Option<R> {
        let mut boxed = self.core.views.get_mut(id)?.view.take()?;
        let result = f(boxed.as_mut(), id, self);
        if let Some(slot) = self.core.views.get_mut(id) {
            slot.view = Some(boxed);
        }
        Some(result)
    }

    pub(crate) fn read_view<V: View>(&self, id: ViewId) -> Option<&V> {
        self.core.views.get(id)?.view.as_ref()?.as_any().downcast_ref()
    }

    /// The window's own view says what it shows again (something
    /// outside the window it shows changed: what may be pasted).
    pub fn invalidate_main_view(&mut self) {
        if let Some(view) = self.core.main_view.or(self.core.root_view) {
            self.core.invalidate_view(view);
        }
    }

    /// Names the view the program knows the window by (see
    /// `main_view`): what its `WindowHandle` reaches.
    pub fn set_main_view<V: View>(&mut self, view: &Child<V>) {
        self.core.main_view = Some(view.id());
    }

    /// Gives the window's own view a command by name (a menu's, the
    /// palette's, an agent's). Whether it was taken.
    pub fn command(&mut self, command: &str) -> bool {
        match self.core.main_view.or(self.core.root_view) {
            Some(view) => self.with_any_view(view, |v, id, cx| v.command(id, command, cx)).unwrap_or(false),
            None => false,
        }
    }

    // ---- The keyboard ----

    /// Gives the keyboard to the first thing in a view that takes it
    /// (once it's shown, if it isn't yet).
    pub(crate) fn focus_view(&mut self, view: ViewId) {
        let found = self.core.views.get(view).and_then(|v| v.root).and_then(|root| self.core.first_focusable(root));
        match found {
            Some(node) => self.focus_node(Some(node)),
            None => {
                self.core.pending_focus = Some(view);
                self.core.invalidate_view(view);
            }
        }
    }

    /// Gives the keyboard to the element a focus handle names (once
    /// it's shown, if it isn't yet).
    pub(crate) fn focus_handle(&mut self, handle: u64) {
        let found = self.core.nodes.iter().find(|(id, n)| n.inter.as_ref().is_some_and(|i| i.focus_handle == Some(handle) && !i.disabled) && self.core.is_attached(*id)).map(|(id, _)| id);
        match found {
            Some(node) => self.focus_node(Some(node)),
            None => {
                self.core.pending_focus_handle = Some(handle);
                self.core.needs_commit = true;
            }
        }
    }

    /// The element a focus handle names gets the keyboard once it's
    /// shown, if nothing has it then.
    pub(crate) fn focus_handle_if_none(&mut self, handle: u64) {
        self.core.fallback_focus = Some(handle);
        self.core.needs_commit = true;
    }

    /// Whether the keyboard is with the element a focus handle names
    /// (or, `within`, something in it).
    pub(crate) fn handle_has_focus(&self, handle: u64, within: bool) -> bool {
        let mut at = self.core.focus;
        while let Some(id) = at {
            let Some(node) = self.core.nodes.get(id) else { break };
            if node.inter.as_ref().is_some_and(|i| i.focus_handle == Some(handle)) {
                return true;
            }
            if !within {
                break;
            }
            at = node.parent;
        }
        false
    }

    /// Whether the keyboard is with a view or something in it.
    pub(crate) fn view_has_focus(&self, view: ViewId) -> bool {
        let Some(root) = self.core.views.get(view).and_then(|v| v.root) else { return false };
        let mut at = self.core.focus;
        while let Some(id) = at {
            if id == root {
                return true;
            }
            at = self.core.nodes.get(id).and_then(|n| n.parent);
        }
        false
    }

    /// Whether the window has a default button now (Return, when what
    /// has the keyboard lets it go on, clicks it).
    pub fn has_default_button(&self) -> bool {
        self.core.window_listeners.iter().any(|id| self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).is_some_and(|i| i.default_key && !i.disabled) && self.core.is_attached(*id))
    }

    /// Tells the input method where the text being typed is (its caret,
    /// in the window): its window opens beside it.
    pub fn set_ime_position(&mut self, bounds: Bounds<Pixels>) {
        self.on_display(move |window| window.update_ime_position(bounds));
    }

    // ---- Windows on this looper ----

    /// Opens a window on this window's own looper, around the view
    /// `build` makes: what belongs to this window and is told things by
    /// it at once (a menu, a tip, an alert), where a window of its own
    /// work gets a looper of its own (`open_window`).
    pub fn open_window_here<V: View>(&mut self, spec: crate::WindowSpec, build: impl FnOnce(&mut Cx<V>) -> V) -> Option<LocalWindow<V>> {
        let id = display().next_window_id();
        let title = spec.title.clone();
        let menu = spec.focus && matches!(spec.kind, crate::WindowKind::Popup { .. });
        match open(self.looper, id, spec.params(), title, build) {
            Ok(id) => {
                let owner = self.core.id;
                if let Some(core) = self.looper.state_mut::<Windows>().and_then(|w| w.map.get_mut(&id)) {
                    core.owner = Some(owner);
                    core.menu = menu;
                }
                Some(LocalWindow { id, _view: std::marker::PhantomData })
            }
            Err(e) => {
                tracing::warn!("a window couldn't be opened: {e:#}");
                None
            }
        }
    }

    /// Nothing more of the window is shown for `wait` at most, or until
    /// [`Self::release_commit`]: what it's becoming (another size, with
    /// what it asked for as it opened) is shown whole, not on its way.
    pub fn hold_commit(&mut self, wait: Duration) {
        let until = Instant::now() + wait;
        if self.core.commit_held.is_none_or(|held| held < until) {
            self.core.commit_held = Some(until);
        }
        // (Shown when the wait is over, whatever came.)
        let id = self.core.id;
        self.looper.after(wait + Duration::from_millis(1), move |looper| {
            with_window(looper, id, |cx| {
                if cx.core.commit_held.is_some_and(|held| held <= Instant::now()) {
                    cx.release_commit();
                }
            });
        });
    }

    /// What was held back is shown ([`Self::hold_commit`]).
    pub fn release_commit(&mut self) {
        if self.core.commit_held.take().is_some() {
            self.core.needs_commit = true;
        }
    }

    /// How app_server holds the window (maximized, full screen, in a
    /// tile).
    pub fn held_state(&self) -> WindowState {
        self.core.state
    }

    /// Whether the window is full screen (as app_server last said). (A
    /// view whose `body` asks says what it shows again when that
    /// changes, as for [`Self::viewport_size`].)
    pub fn is_fullscreen(&self) -> bool {
        SIZE_READ.with(|r| r.set(true));
        self.core.state.fullscreen
    }

    /// Asks for the window full screen, or back: `is_fullscreen` and the
    /// view's next `body` follow once app_server has done it.
    pub fn toggle_fullscreen(&self) {
        self.on_display(|window| window.toggle_fullscreen());
    }

    /// Keeps the screen from dimming, locking or sleeping while the
    /// window shows (a playing video), until called with `false` or the
    /// window closes.
    pub fn set_keep_awake(&self, on: bool) {
        self.on_display(move |window| window.set_keep_awake(on));
    }

    /// Draws the window in `gamut` from its next frame (Display P3 while
    /// it shows a photo wider than sRGB; HDR10, an HDR video): its sRGB
    /// colours converted as drawn, pictures in that space as they are.
    /// With `only_on_hdr`, only if the window is on a display whose HDR
    /// is on. Whether it does (the compositor must take the window's
    /// colours, wp_color_management_v1): asked of the display thread, so
    /// called when what's shown changes, not as it's drawn.
    pub fn set_gamut(&mut self, gamut: crate::Gamut, only_on_hdr: bool) -> bool {
        let id = self.core.id;
        let drawn =
            display().ask(move |on| on.stage(id).and_then(|stage| stage.with_window(|window| (!only_on_hdr || window.on_hdr_display()) && window.set_gamut(gamut))).unwrap_or(false)).unwrap_or(false);
        self.refresh();
        drawn
    }

    /// A handle another program's window can name to open over this
    /// one (xdg-foreign: Quick Look over a file panel), handed to `done`
    /// once the compositor says it, on the display thread; None if
    /// windows can't be exported.
    pub fn export_handle(&self, done: impl FnOnce(Option<String>) + Send + 'static) {
        self.on_display(move |window| window.export_handle(Box::new(done)));
    }

    /// A layer surface's margins from the edges it's anchored to (top,
    /// right, bottom, left; points): how one is moved (the camera bubble,
    /// dragged).
    pub fn set_layer_margin(&self, margin: (i32, i32, i32, i32)) {
        self.on_display(move |window| window.set_layer_margin(margin));
    }

    /// A layer surface takes the keyboard, or doesn't (a strip of
    /// banners takes it only while one has a field to type in).
    pub fn set_layer_keyboard(&self, keyboard: crate::LayerKeyboard) {
        self.on_display(move |window| window.set_layer_keyboard(keyboard));
    }

    /// An activation token made for this window (xdg-activation: what
    /// lets another program bring its window forward because the person
    /// clicked here), handed to `done` on the display thread; None if
    /// the compositor gives none.
    pub fn activation_token(&self, done: impl FnOnce(Option<String>) + Send + 'static) {
        self.on_display(move |window| window.activation_token(Box::new(done)));
    }

    /// Drags `drag` out of the window to another app or a folder
    /// (Wayland's data device), while the press that began it is down.
    /// What comes back resolves, when the drag is over, to what it was
    /// dropped as (None: cancelled).
    pub fn start_drag(&self, drag: crate::OutgoingDrag) -> futures::channel::oneshot::Receiver<Option<crate::DragAction>> {
        let (done, over) = futures::channel::oneshot::channel();
        self.on_display(move |window| window.start_drag(drag, done));
        over
    }

    /// A window with no display (a headless run: the layout lint's
    /// tests) takes this scale, as if a compositor gave it.
    #[doc(hidden)]
    pub fn set_headless_scale(&self, scale: f32) {
        self.on_display(move |window| window.set_headless_scale(scale));
    }

    /// A click at `at` (in the window), as if the person made it there,
    /// once what's running now is done: for showing someone where
    /// something is (a control on a tab that isn't showing: its tab is
    /// chosen as a click would choose it, `olo_kit::spotlight`). The
    /// pointer itself isn't moved, and what's under it is found again
    /// when it next moves.
    pub fn click_at(&mut self, at: Point<Pixels>) {
        let id = self.core.id;
        let (button, modifiers) = (crate::MouseButton::Left, crate::Modifiers::default());
        for input in [
            crate::PlatformInput::MouseMove(crate::MouseMoveEvent { position: at, pressed_button: None, modifiers }),
            crate::PlatformInput::MouseDown(crate::MouseDownEvent { button, position: at, modifiers, click_count: 1, first_mouse: false }),
            crate::PlatformInput::MouseUp(crate::MouseUpEvent { button, position: at, modifiers, click_count: 1 }),
        ] {
            self.looper.defer(move |looper| deliver(looper, id, WindowEvent::Input(input)));
        }
    }

    /// A click at `at` handed to a window with no display, as if the
    /// person made it (the lint clicks each tab).
    #[doc(hidden)]
    pub fn headless_click(&self, at: Point<Pixels>) {
        self.on_display(move |window| {
            let (button, modifiers) = (crate::MouseButton::Left, crate::Modifiers::default());
            window.headless_input(crate::PlatformInput::MouseMove(crate::MouseMoveEvent { position: at, pressed_button: None, modifiers }));
            window.headless_input(crate::PlatformInput::MouseDown(crate::MouseDownEvent { button, position: at, modifiers, click_count: 1, first_mouse: false }));
            window.headless_input(crate::PlatformInput::MouseUp(crate::MouseUpEvent { button, position: at, modifiers, click_count: 1 }));
        });
    }

    /// Shows `buffer` as the picture `key` (another program's memory on
    /// the GPU, an Agent Screen's frame), in place of the one before,
    /// where a drawing placed it (`Painter::place_external`), at once,
    /// with no frame of this window's. The buffer it replaces is
    /// released (`ExternalBuffer::released`) once the compositor is done
    /// with it. None lets `key` go with its buffers.
    pub fn set_external_buffer(&self, key: u64, buffer: Option<crate::ExternalBuffer>) {
        self.on_display(move |window| window.set_external_buffer(key, buffer));
    }

    /// Stops a timer (`Cx::every`, `Cx::after_do`): it doesn't fire
    /// again, or at all.
    pub fn cancel(&mut self, timer: olo_app::TimerId) {
        self.looper.cancel(timer);
    }

    /// Whether files are being dragged over the window from somewhere
    /// else (what would take them, `on_drop`, can show it: it's hovered
    /// as under the pointer meanwhile).
    pub fn files_dragged_over(&self) -> bool {
        self.core.dropping.is_some()
    }

    /// Brings the window forward and gives it the keyboard.
    pub fn activate_window(&mut self) {
        self.on_display(|window| window.activate());
    }

    /// Brings the window forward with a token another program handed
    /// over for it (a notification's action: the notification service's
    /// own, so the compositor lets it take the keyboard).
    pub fn activate_window_with_token(&mut self, token: &str) {
        let token = token.to_owned();
        self.on_display(move |window| window.activate_with_token(&token));
    }

    /// Moves the window with the pointer, as if dragged by its tab,
    /// while a button is held.
    pub fn start_window_move(&mut self) {
        self.on_display(|window| window.start_window_move());
    }

    /// Sends the window's first view (the one it was opened round: a
    /// kit's frame) a message, taken once what's running now is done.
    /// Nothing, if it isn't a `V`.
    pub fn send_root<V: View>(&mut self, message: V::Message) {
        let Some(view) = self.core.root_view else { return };
        let id = self.core.id;
        self.looper.defer(move |looper| deliver_to_view_if::<V>(looper, id, view, |v, cx| v.message(message, cx)));
    }

    /// The displays changed: the window's first view and its own hear.
    pub(crate) fn displays_changed(&mut self) {
        let views: Vec<ViewId> = [self.core.root_view, self.core.main_view].into_iter().flatten().collect();
        for (i, view) in views.iter().enumerate() {
            if i == 0 || views[0] != *view {
                self.with_any_view(*view, |v, id, cx| v.displays_changed(id, cx));
            }
        }
    }

    /// What the window's own view keeps across an upgrade in place
    /// (`View::save_state`).
    pub fn save_state(&mut self) -> Option<String> {
        let view = self.core.main_view.or(self.core.root_view)?;
        self.with_any_view(view, |v, id, cx| v.save_state(id, cx)).flatten()
    }

    /// Asks for the window to be this size (app_server may give
    /// another: a window in a tile keeps its tile's).
    pub fn resize_window(&mut self, size: Size<Pixels>) {
        self.on_display(move |window| window.resize(size));
    }

    // ---- The window ----

    pub fn window_id(&self) -> WindowId {
        self.core.id
    }

    /// The window's content, in points. (A view whose `body` asks says
    /// what it shows again when the window is sized.)
    pub fn viewport_size(&self) -> Size<Pixels> {
        SIZE_READ.with(|r| r.set(true));
        self.core.size
    }

    /// Device pixels to a point on the window's display.
    pub fn scale_factor(&self) -> f32 {
        self.core.gfx.scale_factor
    }

    pub fn rem_size(&self) -> Pixels {
        self.core.gfx.rem_size
    }

    /// How text is drawn for the display the window is on.
    pub fn text_rendering(&self) -> TextRendering {
        self.core.gfx.text_rendering
    }

    pub fn text_system(&self) -> &Arc<WindowTextSystem> {
        &self.core.gfx.text_system
    }

    /// Draws the colour glyphs (emoji) of `texts` in `font` at
    /// `font_size`, for this window's scale, ahead of need and off its
    /// looper, in the order given, so the frames that show them only
    /// copy them. They're kept for every window of the program after
    /// (`TextSystem::forget_kept_glyphs` lets go): a picker of emoji
    /// opened again and again, each time a new window.
    pub fn warm_text(&self, font: &crate::Font, font_size: Pixels, texts: impl Into<Arc<[crate::SharedString]>>) {
        let font_id = self.core.gfx.text_system.resolve_font(font);
        self.core.gfx.text_system.warm_text(font_id, font_size, self.core.gfx.scale_factor, texts.into());
    }

    /// The text style everything in the window starts from.
    pub fn set_text_style(&mut self, style: TextStyle) {
        if self.core.gfx.text_style != style {
            self.core.gfx.text_style = style;
            self.refresh();
        }
    }

    /// Something kept with the window, one of each type (made the first
    /// time it's asked for): a kit's own state for the window.
    pub fn window_state<T: Default + 'static>(&mut self) -> &mut T {
        self.core.kept.entry(std::any::TypeId::of::<T>()).or_insert_with(|| Box::new(T::default())).downcast_mut().expect("kept by its type")
    }

    /// Something kept with the window by `key`, one of each type and
    /// key (made by `make` the first time it's asked for): what an
    /// element built again each time its view says what it shows keeps
    /// between (a control's own fields, its scroller, its keyboard's
    /// name), by the id its view gives it. Kept as long as the window.
    pub fn keyed_state<T: 'static>(&mut self, key: impl Into<SharedString>, make: impl FnOnce(&mut WindowCx) -> T) -> std::rc::Rc<T> {
        let key = (std::any::TypeId::of::<T>(), key.into());
        if let Some(kept) = self.core.keyed.get(&key).cloned().and_then(|kept| kept.downcast::<T>().ok()) {
            return kept;
        }
        let made = std::rc::Rc::new(make(self));
        self.core.keyed.insert(key, made.clone());
        made
    }

    /// Hears what a view emits, from outside any view (what an element
    /// keeps with the window: a control's own fields).
    pub fn subscribe_to<T: View, E: 'static>(&mut self, child: &Child<T>, f: impl Fn(&E, &mut WindowCx) + 'static) {
        let subscriber: crate::view::Subscriber = std::rc::Rc::new(move |event, cx| {
            if let Some(event) = event.downcast_ref::<E>() {
                f(event, cx);
            }
        });
        if let Some(slot) = self.core.views.get_mut(child.id()) {
            slot.subscribers.push((std::any::TypeId::of::<E>(), subscriber));
        }
    }

    /// What the window shows, for agents: everything that says what it
    /// is, in painting order, where it is now and what shows of it.
    /// This is the window's tree itself, kept: nothing is drawn for it.
    pub fn agent_nodes(&self) -> Vec<crate::agent::Node> {
        let mut out = Vec::new();
        if let Some(root) = self.core.root {
            let window = Bounds { origin: Point::default(), size: self.core.size };
            self.core.agent_nodes(root, window, Point::default(), &mut out);
        }
        out
    }

    /// How many times the window has shown something new (for what
    /// keeps something worked out from what shows, to know it's stale).
    pub fn commit_count(&self) -> u64 {
        self.core.commits
    }

    /// Every text that doesn't show in full: where what shows of it is,
    /// and the whole of it.
    pub fn cut_texts(&self) -> Vec<(Bounds<Pixels>, SharedString)> {
        let mut out = Vec::new();
        if let Some(root) = self.core.root {
            let window = Bounds { origin: Point::default(), size: self.core.size };
            self.core.cut_texts(root, window, Point::default(), &mut out);
        }
        out
    }

    /// Puts something with the window for drawings to fill in
    /// (`Painter::ext`): what a layout review collects.
    pub fn set_paint_ext(&mut self, ext: Option<Box<dyn std::any::Any>>) -> Option<Box<dyn std::any::Any>> {
        std::mem::replace(&mut self.core.gfx.ext, ext)
    }

    /// Shows what changed now, not when the looper's queue is next dry
    /// (for what must read the window as it will be: a review).
    pub fn flush_now(&mut self) {
        self.flush();
    }

    /// Whether the window has the keyboard. (A view whose `body` asks
    /// says what it shows again when that changes: what's selected in
    /// it is drawn by it.)
    ///
    /// A window with a menu of its own open is as active as it was: the
    /// menu has the keyboard for it (what's selected in the window, and
    /// what has its keyboard, look as they did while the menu is up, as
    /// in BeOS, where a window's menus never took its activation).
    pub fn is_window_active(&self) -> bool {
        ACTIVE_READ.with(|r| r.set(true));
        self.core.active || self.has_menu_open()
    }

    /// Whether a menu of this window's (or a menu of that one's) is open.
    fn has_menu_open(&self) -> bool {
        let Some(windows) = self.looper.state::<Windows>() else { return false };
        let mut owners = vec![self.core.id];
        let mut found = false;
        // (Menus of menus: a few deep at most.)
        for _ in 0..8 {
            let next: Vec<WindowId> = windows.map.values().filter(|c| c.menu && !c.closed && c.owner.is_some_and(|o| owners.contains(&o))).map(|c| c.id).collect();
            if next.is_empty() {
                break;
            }
            found = true;
            owners = next;
        }
        found
    }

    /// Where the pointer was last seen in the window.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.core.mouse
    }

    pub fn modifiers(&self) -> Modifiers {
        self.core.modifiers
    }

    /// The display the window is on, in points (None until it's known).
    pub fn display_bounds(&self) -> Option<Bounds<Pixels>> {
        self.core.display
    }

    /// Whether the window is a surface of the desktop's own (a layer's,
    /// the lock screen's): it belongs to its display.
    pub fn is_shell_surface(&self) -> bool {
        self.core.shell
    }

    /// The window's `wl_surface` id, which app_server names it by.
    pub fn surface_id(&self) -> Option<u32> {
        self.core.surface_id
    }

    pub fn window_title(&self) -> SharedString {
        self.core.title.clone()
    }

    pub fn set_window_title(&mut self, title: impl Into<SharedString>) {
        let title = title.into();
        if self.core.title != title {
            self.core.title = title.clone();
            self.on_display(move |window| window.set_title(&title));
        }
    }

    /// Has the display thread do something with the window (what only
    /// it can: the surface is its).
    pub(crate) fn on_display(&self, f: impl FnOnce(&mut dyn PlatformWindow) + Send + 'static) {
        let id = self.core.id;
        display().post(move |on| {
            if let Some(stage) = on.stage(id) {
                stage.with_window(f);
            }
        });
    }

    /// Closes the window.
    pub fn close_window(&mut self) {
        self.core.closed = true;
    }

    /// Asks to close the window as its tab's close box does: its own
    /// view is asked first (`View::close_requested`).
    pub fn request_close(&mut self) {
        let id = self.core.id;
        self.looper.defer(move |looper| deliver(looper, id, WindowEvent::CloseRequested));
    }

    /// Where the element whose handler is running now is, in the window.
    pub fn listener_bounds(&self) -> Bounds<Pixels> {
        self.core.current.map(|id| self.core.window_bounds(id)).unwrap_or_default()
    }

    /// The element whose handler is running now, to draw it again later
    /// ([`Self::repaint_node`]) when something it shows from outside the
    /// tree changes (a menu bar's open title, once its menu has gone).
    pub fn listener_node(&self) -> Option<NodeRef> {
        self.core.current.map(NodeRef)
    }

    /// The byte nearest `position` (in the window) of the text the
    /// element whose handler is running is, or the first text it holds:
    /// where a click lands in it. None with no such text laid out.
    pub fn text_index_at(&self, position: Point<Pixels>) -> Option<usize> {
        let mut stack: Vec<NodeId> = self.core.current.into_iter().collect();
        while let Some(id) = stack.pop() {
            let node = self.core.nodes.get(id)?;
            if let NodeKind::Text(text) = &node.kind {
                let laid = text.laid.as_ref()?;
                let origin = self.core.window_bounds(id).origin;
                return Some(crate::tree::text_index_at(laid, point(position.x - origin.x, position.y - origin.y)));
            }
            stack.extend(node.children.iter().rev().copied());
        }
        None
    }

    /// What the element whose handler is running keeps for itself, from
    /// one handler to the next and for its drawing
    /// (`Painter::node_state`).
    pub fn node_state<T: Default + 'static>(&mut self) -> &mut T {
        let slot = match self.core.current.and_then(|id| self.core.nodes.get_mut(id)) {
            Some(node) => &mut node.state,
            // (No element: something to write to, kept nowhere.)
            None => {
                self.core.kept.entry(std::any::TypeId::of::<Option<Box<dyn std::any::Any>>>()).or_insert_with(|| Box::new(None::<Box<dyn std::any::Any>>)).downcast_mut().expect("kept by its type")
            }
        };
        crate::painter::node_state(slot)
    }

    /// Does something as an element's handler would (its state and its
    /// bounds are what `node_state` and `listener_bounds` give), later:
    /// from a frame's step or a timer its handler set going. Nothing, if
    /// the element has gone.
    pub fn with_node(&mut self, node: NodeRef, f: impl FnOnce(&mut WindowCx)) {
        if !self.core.nodes.contains_key(node.0) {
            return;
        }
        let outer = self.core.current.replace(node.0);
        f(self);
        self.core.current = outer;
    }

    /// What the `index`th element held by the one whose handler is
    /// running keeps for itself, to change: it's drawn again. (A control
    /// made of parts drawn apart: a scroll bar's knob over its track.)
    pub fn child_state<T: Default + 'static>(&mut self, index: usize) -> Option<&mut T> {
        let child = self.core.current.and_then(|id| self.core.nodes.get(id)).and_then(|n| n.children.get(index).copied())?;
        self.core.mark_paint(child);
        Some(crate::painter::node_state(&mut self.core.nodes.get_mut(child)?.state))
    }

    /// Draws the element whose handler is running again (what it keeps
    /// for itself changed).
    pub fn repaint(&mut self) {
        if let Some(id) = self.core.current {
            self.core.mark_paint(id);
        }
    }

    /// The view that the element whose handler is running belongs to,
    /// if it's a `V` and isn't busy: for a handler built without the
    /// view to hand that has something to ask of it (a list's rows'
    /// names).
    pub fn owner_view<V: View>(&self) -> Option<&V> {
        let owner = self.core.current.and_then(|id| self.core.nodes.get(id)).map(|n| n.owner)?;
        self.read_view(owner)
    }

    /// Has the view that the element whose handler is running belongs
    /// to say what it shows again.
    pub fn invalidate_owner(&mut self) {
        if let Some(owner) = self.core.current.and_then(|id| self.core.nodes.get(id)).map(|n| n.owner) {
            self.core.invalidate_view(owner);
        }
    }

    /// Draws an element again (if it's still there).
    pub fn repaint_node(&mut self, node: NodeRef) {
        if self.core.nodes.contains_key(node.0) {
            self.core.mark_paint(node.0);
        }
    }

    /// What's being handed round stops here: nothing else gets it.
    pub fn stop_propagation(&mut self) {
        self.core.propagate = false;
    }

    /// What would have happened by default doesn't.
    pub fn prevent_default(&mut self) {
        self.core.default_prevented = true;
    }

    /// Does `f` as the next frame starts: an animation's next step.
    /// (Each step asks for the next: nothing runs when nothing moves.)
    pub fn on_next_frame(&mut self, f: impl FnOnce(&mut WindowCx) + 'static) {
        self.core.next_frame.push(Box::new(f));
        self.core.needs_commit = true;
    }

    /// The looper the window is on.
    pub fn looper(&mut self) -> &mut Looper {
        self.looper
    }

    /// How long ago what's being worked on now was taken (for work that
    /// spreads itself over frames).
    pub fn working_for(&self) -> Duration {
        self.core.started.map(|s| s.elapsed()).unwrap_or_default()
    }
}
