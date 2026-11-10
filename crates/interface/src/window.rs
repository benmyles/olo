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
    stage::{Commit, Item, Layer, Opened, WindowEvent, WindowLink},
    tree::{Node, NodeId, NodeKind, ViewId},
    view::{AnyView, Child, Cx, View, ViewSlot},
};

thread_local! {
    static TEXT: std::cell::RefCell<Option<Arc<WindowTextSystem>>> = const { std::cell::RefCell::new(None) };
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
    pub(crate) gfx: Gfx,
    /// The window's content, in points.
    pub(crate) size: Size<Pixels>,
    pub(crate) title: SharedString,
    pub(crate) surface_id: Option<u32>,
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
    /// Layout is to be worked out again though nothing in the tree says
    /// so (the window's size changed).
    pub(crate) relayout: bool,
    /// The next commit is to be drawn whole.
    pub(crate) everything: bool,
    /// When what's being worked on now was first taken.
    pub(crate) started: Option<Instant>,
    pub(crate) mouse: Point<Pixels>,
    pub(crate) modifiers: Modifiers,
    /// What the pointer is over, topmost first.
    pub(crate) hovered: Vec<NodeId>,
    /// What a button went down on, until it's let go.
    pub(crate) pressed: Vec<(NodeId, MouseDownEvent)>,
    pub(crate) focus: Option<NodeId>,
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
    /// The pointer's shape, as last asked for.
    pub(crate) cursor: crate::CursorStyle,
    /// Scenes let go of, to record into again.
    pub(crate) recycled: Vec<crate::Scene>,
    /// What the kit keeps with the window, one of each type.
    pub(crate) kept: HashMap<std::any::TypeId, Box<dyn std::any::Any>>,
}

/// A window, as whatever is running on its looper is handed it: the
/// window's own state and the looper.
pub struct WindowCx<'a> {
    pub(crate) core: &'a mut WindowCore,
    pub(crate) looper: &'a mut Looper,
}

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
    let result = f(&mut WindowCx { core: &mut core, looper });
    if core.closed {
        core.end(looper);
    } else if let Some(windows) = looper.state_mut::<Windows>() {
        windows.map.insert(id, core);
    }
    Some(result)
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

/// Opens a window on this looper, around the view `build` makes.
pub(crate) fn open<V: View>(looper: &mut Looper, id: WindowId, params: WindowParams, title: SharedString, build: impl FnOnce(&mut Cx<V>) -> V) -> anyhow::Result<WindowId> {
    let display = display();
    let link = WindowLink { looper: looper.handle(), id };
    let opened = display.open_window(id, params, link)?;
    let mut core = Box::new(WindowCore::new(id, opened, title));
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
            gfx,
            size: opened.size,
            title,
            surface_id: opened.surface_id,
            dirty_views: Vec::new(),
            paint_queue: Vec::new(),
            remeasured: Vec::new(),
            needs_commit: true,
            items: Vec::new(),
            items_stale: true,
            relayout: true,
            everything: true,
            started: Some(Instant::now()),
            mouse: Point::default(),
            modifiers: Modifiers::default(),
            hovered: Vec::new(),
            pressed: Vec::new(),
            focus: None,
            window_listeners: Vec::new(),
            active: false,
            closed: false,
            propagate: true,
            default_prevented: false,
            current: None,
            next_frame: Vec::new(),
            cursor: crate::CursorStyle::Arrow,
            recycled: Vec::new(),
            kept: HashMap::new(),
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

    /// A view's owner let go of it: it and what it showed are gone.
    fn end_view(&mut self, view: ViewId) {
        let Some(slot) = self.views.remove(view) else { return };
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
        let mut repaint = resized && (node.behind.is_some() || node.over.is_some());
        if moved && !repaint {
            // What it drew is moved with it, if that's by whole device
            // pixels (it nearly always is: layout lands on them).
            let off = |a: Pixels, b: Pixels| {
                let d = (a.0 - b.0) * scale;
                (d - d.round()).abs() > 0.01
            };
            repaint = off(bounds.origin.x, node.painted_at.x) || off(bounds.origin.y, node.painted_at.y);
        }
        if moved {
            self.reshaped();
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
        self.nodes[id].reach = reach;
        reach
    }

    /// Makes the list of everything that shows, in painting order, for
    /// the stage (and notes where each node's drawings are in it).
    fn list_items(&mut self) {
        let mut items = std::mem::take(&mut self.items);
        items.clear();
        let scale = self.gfx.scale_factor;
        let window = Bounds { origin: point(ScaledPixels(0.), ScaledPixels(0.)), size: size(ScaledPixels(self.size.width.0 * scale), ScaledPixels(self.size.height.0 * scale)) };
        if let Some(root) = self.root {
            self.collect(root, window, &mut items);
        }
        self.items = items;
        self.items_stale = false;
    }

    fn collect(&mut self, id: NodeId, clip: Bounds<ScaledPixels>, items: &mut Vec<Item>) {
        let scale = self.gfx.scale_factor;
        let rem = self.gfx.rem_size;
        let Some(node) = self.nodes.get_mut(id) else { return };
        node.slots = [u32::MAX; 2];
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        let by = point(ScaledPixels(((node.bounds.origin.x.0 - node.painted_at.x.0) * scale).round()), ScaledPixels(((node.bounds.origin.y.0 - node.painted_at.y.0) * scale).round()));
        let shows = |list: &Arc<crate::stage::Recording>| !Bounds { origin: list.bounds.origin + by, size: list.bounds.size }.intersect(&clip).is_empty();
        if let Some(list) = &node.behind
            && shows(list)
        {
            node.slots[0] = items.len() as u32;
            items.push(Item { list: list.clone(), by, clip, layer: 0 });
        }
        let count = node.children.len();
        if count > 0 {
            let inner = match node.style.overflow_mask(node.bounds, rem) {
                Some(mask) => clip.intersect(&mask.scale(scale).bounds),
                None => clip,
            };
            if !inner.is_empty() {
                for i in 0..count {
                    let child = self.nodes[id].children[i];
                    self.collect(child, inner, items);
                }
            }
        }
        let node = &mut self.nodes[id];
        if let Some(list) = &node.over
            && shows(list)
        {
            node.slots[1] = items.len() as u32;
            items.push(Item { list: list.clone(), by, clip, layer: 0 });
        }
    }

    /// A node drew again and nothing else about it changed: its place in
    /// the list of what shows takes the new drawing. False if the list
    /// has to be made again (it now draws something where it drew
    /// nothing, or the other way).
    fn patch_items(&mut self, id: NodeId) -> bool {
        let Some(node) = self.nodes.get(id) else { return true };
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

    fn agent_nodes(&self, id: NodeId, clip: Bounds<Pixels>, out: &mut Vec<crate::agent::Node>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        // What it is itself, if it says.
        if let Some(inter) = &node.inter
            && let Some(agent) = &inter.agent
        {
            out.push(crate::agent::Node {
                role: agent.role,
                name: agent.name.clone(),
                value: agent.value.clone(),
                bounds: node.bounds,
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
            let moved = point(node.bounds.origin.x - node.painted_at.x, node.bounds.origin.y - node.painted_at.y);
            for noted in &notes.agent {
                let mut noted = noted.clone();
                noted.bounds.origin = noted.bounds.origin + moved;
                noted.clip = Some(noted.clip.map_or(clip, |c| Bounds { origin: c.origin + moved, size: c.size }.intersect(&clip)));
                out.push(noted);
            }
        }
        let inner = match node.style.overflow_mask(node.bounds, self.gfx.rem_size) {
            Some(mask) => clip.intersect(&mask.bounds),
            None => clip,
        };
        for child in &node.children {
            self.agent_nodes(*child, inner, out);
        }
    }

    fn cut_texts(&self, id: NodeId, out: &mut Vec<(Bounds<Pixels>, SharedString)>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        if let Some(notes) = &node.notes {
            let moved = point(node.bounds.origin.x - node.painted_at.x, node.bounds.origin.y - node.painted_at.y);
            out.extend(notes.cuts.iter().map(|(b, full)| (Bounds { origin: b.origin + moved, size: b.size }, full.clone())));
        }
        for child in &node.children {
            self.cut_texts(*child, out);
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
            core.relayout = true;
            core.reshaped();
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
        let flushing = Instant::now();
        let phases = crate::stats::frame_phases();
        // What changed says what it shows now.
        let mut turns = 0;
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
                // (Not shown now: it says what it shows when it's placed
                // again, `place_view`.)
                continue;
            }
            let Some(mut boxed) = self.core.views[view].view.take() else { continue };
            let mut element = boxed.body(view, self);
            if let Some(slot) = self.core.views.get_mut(view) {
                slot.view = Some(boxed);
            }
            if matches!(element.kind, crate::element::Kind::View(_)) {
                // (A view that is only another view: held in a box of
                // its own, as the node is this view's.)
                use crate::{ParentElement as _, Styled as _};
                element = crate::div().size_full().child(element);
            }
            if self.core.nodes.contains_key(root) {
                self.core.reconcile(root, element, view);
            }
        }
        let core = &mut *self.core;
        let bodies_done = flushing.elapsed();
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
                for id in std::mem::take(&mut core.remeasured) {
                    core.mark_paint(id);
                }
            }
        }
        let layout_done = flushing.elapsed();
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
        let paint_done = flushing.elapsed();
        if !core.needs_commit {
            core.started = None;
            return;
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
        let layers = vec![Layer {
            id: 0,
            parent: 0,
            viewport: Bounds { origin: point(ScaledPixels(0.), ScaledPixels(0.)), size: size(ScaledPixels(core.size.width.0 * scale), ScaledPixels(core.size.height.0 * scale)) },
            range: Size::default(),
            offset: None,
        }];
        core.commits += 1;
        let started = core.started.take().unwrap_or(flushing);
        let commit = Commit { size: core.size, scale, items, layers, started, work: started.elapsed(), everything: std::mem::take(&mut core.everything), animating: !core.next_frame.is_empty() };
        let id = core.id;
        display().post(move |on| {
            if let Some(stage) = on.stage(id) {
                stage.commit(commit);
            }
        });
    }

    // ---- Views ----

    /// Makes a view in this window; whoever keeps the [`Child`] owns it.
    pub fn new_view<T: View>(&mut self, build: impl FnOnce(&mut Cx<T>) -> T) -> Child<T> {
        let id = self.core.views.insert(ViewSlot { view: None, root: None, dirty: false, subscribers: Vec::new(), timers: Vec::new() });
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

    // ---- Windows on this looper ----

    /// Opens a window on this window's own looper, around the view
    /// `build` makes: what belongs to this window and is told things by
    /// it at once (a menu, a tip, an alert), where a window of its own
    /// work gets a looper of its own (`open_window`).
    pub fn open_window_here<V: View>(&mut self, spec: crate::WindowSpec, build: impl FnOnce(&mut Cx<V>) -> V) -> Option<LocalWindow<V>> {
        let id = display().next_window_id();
        let title = spec.title.clone();
        match open(self.looper, id, spec.params(), title, build) {
            Ok(id) => {
                let owner = self.core.id;
                if let Some(core) = self.looper.state_mut::<Windows>().and_then(|w| w.map.get_mut(&id)) {
                    core.owner = Some(owner);
                }
                Some(LocalWindow { id, _view: std::marker::PhantomData })
            }
            Err(e) => {
                tracing::warn!("a window couldn't be opened: {e:#}");
                None
            }
        }
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

    /// The window's content, in points.
    pub fn viewport_size(&self) -> Size<Pixels> {
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

    /// What the window shows, for agents: everything that says what it
    /// is, in painting order, where it is now and what shows of it.
    /// This is the window's tree itself, kept: nothing is drawn for it.
    pub fn agent_nodes(&self) -> Vec<crate::agent::Node> {
        let mut out = Vec::new();
        if let Some(root) = self.core.root {
            let window = Bounds { origin: Point::default(), size: self.core.size };
            self.core.agent_nodes(root, window, &mut out);
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
            self.core.cut_texts(root, &mut out);
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

    /// Whether the window has the keyboard.
    pub fn is_window_active(&self) -> bool {
        self.core.active
    }

    /// Where the pointer was last seen in the window.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.core.mouse
    }

    pub fn modifiers(&self) -> Modifiers {
        self.core.modifiers
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

    /// Where the element whose handler is running now is, in the window.
    pub fn listener_bounds(&self) -> Bounds<Pixels> {
        self.core.current.and_then(|id| self.core.nodes.get(id)).map(|n| n.bounds).unwrap_or_default()
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
