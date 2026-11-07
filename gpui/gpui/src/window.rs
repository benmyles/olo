#[cfg(any(feature = "inspector", debug_assertions))]
use crate::Inspector;
use crate::{
    Action, AnyDrag, AnyElement, AnyImageCache, AnyTooltip, AnyView, App, AppContext, Arena, Asset,
    AsyncWindowContext, AtlasTile, AvailableSpace, Background, BorderStyle, Bounds, BoxShadow,
    Capslock, Context, Corners, CursorStyle, Decorations, DevicePixels, DispatchActionListener,
    DispatchNodeId, DispatchTree, DisplayId, Edges, Effect, Entity, EntityId, EventEmitter,
    FileDropEvent, FontId, Global, GlobalElementId, GlyphId, GpuSpecs, Hsla, InputHandler, IsZero,
    KeyBinding, KeyContext, KeyDownEvent, KeyEvent, Keystroke, KeystrokeEvent, LayoutId,
    LineLayoutIndex, Modifiers, ModifiersChangedEvent, MonochromeSprite, MouseButton, MouseEvent,
    MouseMoveEvent, MouseUpEvent, Path, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput,
    PlatformInputHandler, PlatformWindow, Point, PolychromeSprite, PromptButton, PromptLevel, Quad,
    Render, RenderGlyphParams, RenderImage, RenderImageParams, RenderSvgParams, Replay, ResizeEdge,
    SMOOTH_SVG_SCALE_FACTOR, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, ScaledPixels, Scene, Sector,
    Segment, Shadow, SharedString, Size, StrikethroughStyle, Style, SubscriberSet, Subscription,
    SystemWindowTab, SystemWindowTabController, TabStopMap, TaffyLayoutEngine, Task, TextRendering,
    TextStyle, TextStyleRefinement, TransformationMatrix, Underline, UnderlineStyle,
    WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControls, WindowDecorations,
    WindowOptions, WindowParams, WindowTextSystem, point, prelude::*, px, rems, size,
    transparent_black,
};
use anyhow::{Context as _, Result, anyhow};
use collections::{FxHashMap, FxHashSet};
#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;
use derive_more::{Deref, DerefMut};
use futures::FutureExt;
use futures::channel::oneshot;
use itertools::FoldWhile::{Continue, Done};
use itertools::Itertools;
use parking_lot::RwLock;
use raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use refineable::Refineable;
use slotmap::SlotMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{DerefMut, Range},
    rc::Rc,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use util::measure;
use util::post_inc;
use uuid::Uuid;

mod prompts;

use crate::util::atomic_incr_if_not_zero;
pub use prompts::*;

pub(crate) const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(864.));

/// Olo: how a window kept up, for agents and tools (the kit's
/// `{"frames":true}`): frames drawn since last taken, their total and
/// slowest time from starting to draw to handing the frame to the GPU,
/// and the last [`FrameStats::MARKS`] of them.
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// Frames drawn.
    pub frames: u32,
    /// Their time together, in microseconds.
    pub total_us: u64,
    /// The slowest one's.
    pub slowest_us: u64,
    /// Each frame: when it started (CLOCK_MONOTONIC microseconds), how
    /// long laying out and painting took, and the whole frame.
    pub marks: Vec<(u64, u32, u32)>,
    /// Olo: each mark's scroll views, as they were drawn in it (where
    /// each showed its content from, x and y, and the furthest it goes):
    /// [`Window::note_scroll`].
    pub scrolls: Vec<Vec<[f32; 4]>>,
}

impl FrameStats {
    /// How many frames' marks are kept between takes.
    pub const MARKS: usize = 1200;

    fn record(
        &mut self,
        started: Instant,
        drawn: Duration,
        whole: Duration,
        scrolls: Vec<[f32; 4]>,
    ) {
        let us = whole.as_micros() as u64;
        self.frames += 1;
        self.total_us += us;
        self.slowest_us = self.slowest_us.max(us);
        if self.marks.len() == Self::MARKS {
            self.marks.remove(0);
            self.scrolls.remove(0);
        }
        let at = monotonic_us().saturating_sub(started.elapsed().as_micros() as u64);
        self.marks.push((at, drawn.as_micros() as u32, us as u32));
        self.scrolls.push(scrolls);
    }
}

fn monotonic_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid timespec for the kernel to fill.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000 + ts.tv_nsec as u64 / 1_000
}

/// Represents the two different phases when dispatching events.
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub enum DispatchPhase {
    /// After the capture phase comes the bubble phase, in which mouse event listeners are
    /// invoked front to back and keyboard event listeners are invoked from the focused element
    /// to the root of the element tree. This is the phase you'll most commonly want to use when
    /// registering event listeners.
    #[default]
    Bubble,
    /// During the initial capture phase, mouse event listeners are invoked back to front, and keyboard
    /// listeners are invoked from the root of the tree downward toward the focused element. This phase
    /// is used for special purposes such as clearing the "pressed" state for click events. If
    /// you stop event propagation during this phase, you need to know what you're doing. Handlers
    /// outside of the immediate region may rely on detecting non-local events during this phase.
    Capture,
}

impl DispatchPhase {
    /// Returns true if this represents the "bubble" phase.
    #[inline]
    pub fn bubble(self) -> bool {
        self == DispatchPhase::Bubble
    }

    /// Returns true if this represents the "capture" phase.
    #[inline]
    pub fn capture(self) -> bool {
        self == DispatchPhase::Capture
    }
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    /// Olo: tells the platform window there's something new to draw.
    pub waker: Option<Rc<dyn Fn()>>,
}

#[derive(Clone)]
pub(crate) struct WindowInvalidator {
    inner: Rc<RefCell<WindowInvalidatorInner>>,
}

impl WindowInvalidator {
    pub fn new() -> Self {
        WindowInvalidator {
            inner: Rc::new(RefCell::new(WindowInvalidatorInner {
                dirty: true,
                draw_phase: DrawPhase::None,
                dirty_views: FxHashSet::default(),
                waker: None,
            })),
        }
    }

    pub fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.dirty_views.insert(entity);
        if inner.draw_phase == DrawPhase::None {
            let woke = !inner.dirty;
            inner.dirty = true;
            let waker = inner.waker.clone().filter(|_| woke);
            drop(inner);
            if let Some(waker) = waker {
                waker();
            }
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    pub fn set_dirty(&self, dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        let woke = dirty && !inner.dirty;
        inner.dirty = dirty;
        let waker = inner.waker.clone().filter(|_| woke);
        drop(inner);
        if let Some(waker) = waker {
            waker();
        }
    }

    pub fn set_waker(&self, waker: Option<Rc<dyn Fn()>>) {
        self.inner.borrow_mut().waker = waker;
    }

    /// Asks for a frame though nothing is dirty (a frame callback queued).
    pub fn wake(&self) {
        let waker = self.inner.borrow().waker.clone();
        if let Some(waker) = waker {
            waker();
        }
    }

    pub fn set_phase(&self, phase: DrawPhase) {
        self.inner.borrow_mut().draw_phase = phase
    }

    pub fn take_views(&self) -> FxHashSet<EntityId> {
        mem::take(&mut self.inner.borrow_mut().dirty_views)
    }

    pub fn replace_views(&self, views: FxHashSet<EntityId>) {
        self.inner.borrow_mut().dirty_views = views;
    }

    pub fn not_drawing(&self) -> bool {
        self.inner.borrow().draw_phase == DrawPhase::None
    }

    #[track_caller]
    pub fn debug_assert_paint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Paint),
            "this method can only be called during paint"
        );
    }

    #[track_caller]
    pub fn debug_assert_prepaint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Prepaint),
            "this method can only be called during request_layout, or prepaint"
        );
    }

    #[track_caller]
    pub fn debug_assert_paint_or_prepaint(&self) {
        debug_assert!(
            matches!(
                self.inner.borrow().draw_phase,
                DrawPhase::Paint | DrawPhase::Prepaint
            ),
            "this method can only be called during request_layout, prepaint, or paint"
        );
    }
}

type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type AnyWindowFocusListener =
    Box<dyn FnMut(&WindowFocusEvent, &mut Window, &mut App) -> bool + 'static>;

pub(crate) struct WindowFocusEvent {
    pub(crate) previous_focus_path: SmallVec<[FocusId; 8]>,
    pub(crate) current_focus_path: SmallVec<[FocusId; 8]>,
}

impl WindowFocusEvent {
    pub fn is_focus_in(&self, focus_id: FocusId) -> bool {
        !self.previous_focus_path.contains(&focus_id) && self.current_focus_path.contains(&focus_id)
    }

    pub fn is_focus_out(&self, focus_id: FocusId) -> bool {
        self.previous_focus_path.contains(&focus_id) && !self.current_focus_path.contains(&focus_id)
    }
}

/// This is provided when subscribing for `Context::on_focus_out` events.
pub struct FocusOutEvent {
    /// A weak focus handle representing what was blurred.
    pub blurred: WeakFocusHandle,
}

slotmap::new_key_type! {
    /// A globally unique identifier for a focusable element.
    pub struct FocusId;
}

thread_local! {
    pub(crate) static ELEMENT_ARENA: RefCell<Arena> = RefCell::new(Arena::new(1024 * 1024));
}

/// Returned when the element arena has been used and so must be cleared before the next draw.
#[must_use]
pub struct ArenaClearNeeded;

impl ArenaClearNeeded {
    /// Clear the element arena.
    pub fn clear(self) {
        ELEMENT_ARENA.with_borrow_mut(|element_arena| {
            element_arena.clear();
        });
    }
}

pub(crate) type FocusMap = RwLock<SlotMap<FocusId, FocusRef>>;
pub(crate) struct FocusRef {
    pub(crate) ref_count: AtomicUsize,
    pub(crate) tab_index: isize,
    pub(crate) tab_stop: bool,
}

impl FocusId {
    /// Obtains whether the element associated with this handle is currently focused.
    pub fn is_focused(&self, window: &Window) -> bool {
        window.focus == Some(*self)
    }

    /// Obtains whether the element associated with this handle contains the focused
    /// element or is itself focused.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        window
            .focused(cx)
            .is_some_and(|focused| self.contains(focused.id, window))
    }

    /// Obtains whether the element associated with this handle is contained within the
    /// focused element or is itself focused.
    pub fn within_focused(&self, window: &Window, cx: &App) -> bool {
        let focused = window.focused(cx);
        focused.is_some_and(|focused| focused.id.contains(*self, window))
    }

    /// Obtains whether this handle contains the given handle in the most recently rendered frame.
    pub(crate) fn contains(&self, other: Self, window: &Window) -> bool {
        window
            .rendered_frame
            .dispatch_tree
            .focus_contains(*self, other)
    }
}

/// A handle which can be used to track and manipulate the focused element in a window.
pub struct FocusHandle {
    pub(crate) id: FocusId,
    handles: Arc<FocusMap>,
    /// The index of this element in the tab order.
    pub tab_index: isize,
    /// Whether this element can be focused by tab navigation.
    pub tab_stop: bool,
}

impl std::fmt::Debug for FocusHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("FocusHandle({:?})", self.id))
    }
}

impl FocusHandle {
    pub(crate) fn new(handles: &Arc<FocusMap>) -> Self {
        let id = handles.write().insert(FocusRef {
            ref_count: AtomicUsize::new(1),
            tab_index: 0,
            tab_stop: false,
        });

        Self {
            id,
            tab_index: 0,
            tab_stop: false,
            handles: handles.clone(),
        }
    }

    pub(crate) fn for_id(id: FocusId, handles: &Arc<FocusMap>) -> Option<Self> {
        let lock = handles.read();
        let focus = lock.get(id)?;
        if atomic_incr_if_not_zero(&focus.ref_count) == 0 {
            return None;
        }
        Some(Self {
            id,
            tab_index: focus.tab_index,
            tab_stop: focus.tab_stop,
            handles: handles.clone(),
        })
    }

    /// Sets the tab index of the element associated with this handle.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        if let Some(focus) = self.handles.write().get_mut(self.id) {
            focus.tab_index = index;
        }
        self
    }

    /// Sets whether the element associated with this handle is a tab stop.
    ///
    /// When `false`, the element will not be included in the tab order.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        if let Some(focus) = self.handles.write().get_mut(self.id) {
            focus.tab_stop = tab_stop;
        }
        self
    }

    /// Converts this focus handle into a weak variant, which does not prevent it from being released.
    pub fn downgrade(&self) -> WeakFocusHandle {
        WeakFocusHandle {
            id: self.id,
            handles: Arc::downgrade(&self.handles),
        }
    }

    /// Moves the focus to the element associated with this handle.
    pub fn focus(&self, window: &mut Window) {
        window.focus(self)
    }

    /// Obtains whether the element associated with this handle is currently focused.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.id.is_focused(window)
    }

    /// Obtains whether the element associated with this handle contains the focused
    /// element or is itself focused.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.id.contains_focused(window, cx)
    }

    /// Obtains whether the element associated with this handle is contained within the
    /// focused element or is itself focused.
    pub fn within_focused(&self, window: &Window, cx: &mut App) -> bool {
        self.id.within_focused(window, cx)
    }

    /// Obtains whether this handle contains the given handle in the most recently rendered frame.
    pub fn contains(&self, other: &Self, window: &Window) -> bool {
        self.id.contains(other.id, window)
    }

    /// Dispatch an action on the element that rendered this focus handle
    pub fn dispatch_action(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        if let Some(node_id) = window
            .rendered_frame
            .dispatch_tree
            .focusable_node_id(self.id)
        {
            window.dispatch_action_on_node(node_id, action, cx)
        }
    }
}

impl Clone for FocusHandle {
    fn clone(&self) -> Self {
        Self::for_id(self.id, &self.handles).unwrap()
    }
}

impl PartialEq for FocusHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for FocusHandle {}

impl Drop for FocusHandle {
    fn drop(&mut self) {
        self.handles
            .read()
            .get(self.id)
            .unwrap()
            .ref_count
            .fetch_sub(1, SeqCst);
    }
}

/// A weak reference to a focus handle.
#[derive(Clone, Debug)]
pub struct WeakFocusHandle {
    pub(crate) id: FocusId,
    pub(crate) handles: Weak<FocusMap>,
}

impl WeakFocusHandle {
    /// Attempts to upgrade the [WeakFocusHandle] to a [FocusHandle].
    pub fn upgrade(&self) -> Option<FocusHandle> {
        let handles = self.handles.upgrade()?;
        FocusHandle::for_id(self.id, &handles)
    }
}

impl PartialEq for WeakFocusHandle {
    fn eq(&self, other: &WeakFocusHandle) -> bool {
        self.id == other.id
    }
}

impl Eq for WeakFocusHandle {}

impl PartialEq<FocusHandle> for WeakFocusHandle {
    fn eq(&self, other: &FocusHandle) -> bool {
        self.id == other.id
    }
}

impl PartialEq<WeakFocusHandle> for FocusHandle {
    fn eq(&self, other: &WeakFocusHandle) -> bool {
        self.id == other.id
    }
}

/// Focusable allows users of your view to easily
/// focus it (using window.focus_view(cx, view))
pub trait Focusable: 'static {
    /// Returns the focus handle associated with this view.
    fn focus_handle(&self, cx: &App) -> FocusHandle;
}

impl<V: Focusable> Focusable for Entity<V> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }
}

/// ManagedView is a view (like a Modal, Popover, Menu, etc.)
/// where the lifecycle of the view is handled by another view.
pub trait ManagedView: Focusable + EventEmitter<DismissEvent> + Render {}

impl<M: Focusable + EventEmitter<DismissEvent> + Render> ManagedView for M {}

/// Emitted by implementers of [`ManagedView`] to indicate the view should be dismissed, such as when a view is presented as a modal.
pub struct DismissEvent;

type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;

pub(crate) type AnyMouseListener =
    Box<dyn FnMut(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static>;

#[derive(Clone)]
pub(crate) struct CursorStyleRequest {
    pub(crate) hitbox_id: Option<HitboxId>,
    pub(crate) style: CursorStyle,
}

#[derive(Default, Eq, PartialEq)]
pub(crate) struct HitTest {
    pub(crate) ids: SmallVec<[HitboxId; 8]>,
    pub(crate) hover_hitbox_count: usize,
}

/// A type of window control area that corresponds to the platform window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowControlArea {
    /// An area that allows dragging of the platform window.
    Drag,
    /// An area that allows closing of the platform window.
    Close,
    /// An area that allows maximizing of the platform window.
    Max,
    /// An area that allows minimizing of the platform window.
    Min,
}

/// An identifier for a [Hitbox] which also includes [HitboxBehavior].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct HitboxId(u64);

impl HitboxId {
    /// Checks if the hitbox with this ID is currently hovered. Except when handling
    /// `ScrollWheelEvent`, this is typically what you want when determining whether to handle mouse
    /// events or paint hover styles.
    ///
    /// See [`Hitbox::is_hovered`] for details.
    pub fn is_hovered(self, window: &Window) -> bool {
        let hit_test = &window.mouse_hit_test;
        for id in hit_test.ids.iter().take(hit_test.hover_hitbox_count) {
            if self == *id {
                return true;
            }
        }
        false
    }

    /// Checks if the hitbox with this ID contains the mouse and should handle scroll events.
    /// Typically this should only be used when handling `ScrollWheelEvent`, and otherwise
    /// `is_hovered` should be used. See the documentation of `Hitbox::is_hovered` for details about
    /// this distinction.
    pub fn should_handle_scroll(self, window: &Window) -> bool {
        window.mouse_hit_test.ids.contains(&self)
    }

    fn next(mut self) -> HitboxId {
        HitboxId(self.0.wrapping_add(1))
    }
}

/// A rectangular region that potentially blocks hitboxes inserted prior.
/// See [Window::insert_hitbox] for more details.
#[derive(Clone, Debug, Deref)]
pub struct Hitbox {
    /// A unique identifier for the hitbox.
    pub id: HitboxId,
    /// The bounds of the hitbox.
    #[deref]
    pub bounds: Bounds<Pixels>,
    /// The content mask when the hitbox was inserted.
    pub content_mask: ContentMask<Pixels>,
    /// Flags that specify hitbox behavior.
    pub behavior: HitboxBehavior,
}

impl Hitbox {
    /// Checks if the hitbox is currently hovered. Except when handling `ScrollWheelEvent`, this is
    /// typically what you want when determining whether to handle mouse events or paint hover
    /// styles.
    ///
    /// This can return `false` even when the hitbox contains the mouse, if a hitbox in front of
    /// this sets `HitboxBehavior::BlockMouse` (`InteractiveElement::occlude`) or
    /// `HitboxBehavior::BlockMouseExceptScroll` (`InteractiveElement::block_mouse_except_scroll`).
    ///
    /// Handling of `ScrollWheelEvent` should typically use `should_handle_scroll` instead.
    /// Concretely, this is due to use-cases like overlays that cause the elements under to be
    /// non-interactive while still allowing scrolling. More abstractly, this is because
    /// `is_hovered` is about element interactions directly under the mouse - mouse moves, clicks,
    /// hover styling, etc. In contrast, scrolling is about finding the current outer scrollable
    /// container.
    pub fn is_hovered(&self, window: &Window) -> bool {
        self.id.is_hovered(window)
    }

    /// Checks if the hitbox contains the mouse and should handle scroll events. Typically this
    /// should only be used when handling `ScrollWheelEvent`, and otherwise `is_hovered` should be
    /// used. See the documentation of `Hitbox::is_hovered` for details about this distinction.
    ///
    /// This can return `false` even when the hitbox contains the mouse, if a hitbox in front of
    /// this sets `HitboxBehavior::BlockMouse` (`InteractiveElement::occlude`).
    pub fn should_handle_scroll(&self, window: &Window) -> bool {
        self.id.should_handle_scroll(window)
    }
}

/// How the hitbox affects mouse behavior.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum HitboxBehavior {
    /// Normal hitbox mouse behavior, doesn't affect mouse handling for other hitboxes.
    #[default]
    Normal,

    /// All hitboxes behind this hitbox will be ignored and so will have `hitbox.is_hovered() ==
    /// false` and `hitbox.should_handle_scroll() == false`. Typically for elements this causes
    /// skipping of all mouse events, hover styles, and tooltips. This flag is set by
    /// [`InteractiveElement::occlude`].
    ///
    /// For mouse handlers that check those hitboxes, this behaves the same as registering a
    /// bubble-phase handler for every mouse event type:
    ///
    /// ```ignore
    /// window.on_mouse_event(move |_: &EveryMouseEventTypeHere, phase, window, cx| {
    ///     if phase == DispatchPhase::Capture && hitbox.is_hovered(window) {
    ///         cx.stop_propagation();
    ///     }
    /// })
    /// ```
    ///
    /// This has effects beyond event handling - any use of hitbox checking, such as hover
    /// styles and tooltops. These other behaviors are the main point of this mechanism. An
    /// alternative might be to not affect mouse event handling - but this would allow
    /// inconsistent UI where clicks and moves interact with elements that are not considered to
    /// be hovered.
    BlockMouse,

    /// All hitboxes behind this hitbox will have `hitbox.is_hovered() == false`, even when
    /// `hitbox.should_handle_scroll() == true`. Typically for elements this causes all mouse
    /// interaction except scroll events to be ignored - see the documentation of
    /// [`Hitbox::is_hovered`] for details. This flag is set by
    /// [`InteractiveElement::block_mouse_except_scroll`].
    ///
    /// For mouse handlers that check those hitboxes, this behaves the same as registering a
    /// bubble-phase handler for every mouse event type **except** `ScrollWheelEvent`:
    ///
    /// ```ignore
    /// window.on_mouse_event(move |_: &EveryMouseEventTypeExceptScroll, phase, window, cx| {
    ///     if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
    ///         cx.stop_propagation();
    ///     }
    /// })
    /// ```
    ///
    /// See the documentation of [`Hitbox::is_hovered`] for details of why `ScrollWheelEvent` is
    /// handled differently than other mouse events. If also blocking these scroll events is
    /// desired, then a `cx.stop_propagation()` handler like the one above can be used.
    ///
    /// This has effects beyond event handling - this affects any use of `is_hovered`, such as
    /// hover styles and tooltops. These other behaviors are the main point of this mechanism.
    /// An alternative might be to not affect mouse event handling - but this would allow
    /// inconsistent UI where clicks and moves interact with elements that are not considered to
    /// be hovered.
    BlockMouseExceptScroll,
}

/// An identifier for a tooltip.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct TooltipId(usize);

impl TooltipId {
    /// Checks if the tooltip is currently hovered.
    pub fn is_hovered(&self, window: &Window) -> bool {
        window
            .tooltip_bounds
            .as_ref()
            .is_some_and(|tooltip_bounds| {
                tooltip_bounds.id == *self
                    && tooltip_bounds.bounds.contains(&window.mouse_position())
            })
    }
}

pub(crate) struct TooltipBounds {
    id: TooltipId,
    bounds: Bounds<Pixels>,
}

#[derive(Clone)]
pub(crate) struct TooltipRequest {
    id: TooltipId,
    tooltip: AnyTooltip,
}

pub(crate) struct DeferredDraw {
    current_view: EntityId,
    priority: usize,
    parent_node: DispatchNodeId,
    element_id_stack: SmallVec<[ElementId; 32]>,
    text_style_stack: Vec<TextStyleRefinement>,
    element: Option<AnyElement>,
    absolute_offset: Point<Pixels>,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    accessed_element_states: Vec<(GlobalElementId, TypeId)>,
    pub(crate) mouse_listeners: Vec<Option<AnyMouseListener>>,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) scene: Scene,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    pub(crate) tooltip_requests: Vec<Option<TooltipRequest>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
    /// Olo: what the app noted as it painted (`Window::push_paint_record`),
    /// replayed with a cached view's paint; each with how far it has been
    /// moved since (`Window::paint_memo`).
    pub(crate) paint_records: Vec<(Rc<dyn Any>, Point<Pixels>)>,
    /// Olo: what was painted under a key (`Window::paint_memo`), in the
    /// order painted (a cached view's paint replayed brings the memos
    /// painted in it along: `PaintIndex::memos_index`), and where each
    /// key's is.
    pub(crate) memos: Vec<(u64, PaintMemo)>,
    pub(crate) memo_index: FxHashMap<u64, usize>,
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) debug_bounds: FxHashMap<String, Bounds<Pixels>>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) next_inspector_instance_ids: FxHashMap<Rc<crate::InspectorElementPath>, usize>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) inspector_hitboxes: FxHashMap<HitboxId, crate::InspectorElementId>,
    pub(crate) tab_stops: TabStopMap,
}

#[derive(Clone, Default)]
pub(crate) struct PrepaintStateIndex {
    hitboxes_index: usize,
    tooltips_index: usize,
    deferred_draws_index: usize,
    dispatch_tree_index: usize,
    accessed_element_states_index: usize,
    line_layout_index: LineLayoutIndex,
}

/// Olo: where an external picture was placed (`Window::place_external`),
/// kept as a paint record so a cached view's replay brings it along.
struct ExternalPlace {
    key: u64,
    bounds: Bounds<Pixels>,
}

#[derive(Clone, Default)]
pub(crate) struct PaintIndex {
    scene_index: usize,
    mouse_listeners_index: usize,
    input_handlers_index: usize,
    cursor_styles_index: usize,
    accessed_element_states_index: usize,
    tab_handle_index: usize,
    line_layout_index: LineLayoutIndex,
    paint_records_index: usize,
    memos_index: usize,
}

impl PaintIndex {
    /// Olo: where this index of what was painted from `from` is, painted
    /// again from `to` (replayed in place: one for one).
    fn shifted(&self, from: &PaintIndex, to: &PaintIndex) -> PaintIndex {
        let at = |v: usize, f: usize, t: usize| v - f + t;
        PaintIndex {
            scene_index: at(self.scene_index, from.scene_index, to.scene_index),
            mouse_listeners_index: at(
                self.mouse_listeners_index,
                from.mouse_listeners_index,
                to.mouse_listeners_index,
            ),
            input_handlers_index: at(
                self.input_handlers_index,
                from.input_handlers_index,
                to.input_handlers_index,
            ),
            cursor_styles_index: at(
                self.cursor_styles_index,
                from.cursor_styles_index,
                to.cursor_styles_index,
            ),
            accessed_element_states_index: at(
                self.accessed_element_states_index,
                from.accessed_element_states_index,
                to.accessed_element_states_index,
            ),
            tab_handle_index: at(
                self.tab_handle_index,
                from.tab_handle_index,
                to.tab_handle_index,
            ),
            line_layout_index: self
                .line_layout_index
                .shifted(&from.line_layout_index, &to.line_layout_index),
            paint_records_index: at(
                self.paint_records_index,
                from.paint_records_index,
                to.paint_records_index,
            ),
            memos_index: at(self.memos_index, from.memos_index, to.memos_index),
        }
    }
}

impl Frame {
    /// Olo: notes `memo` as painted under `key`.
    fn keep_memo(&mut self, key: u64, memo: PaintMemo) {
        self.memo_index.insert(key, self.memos.len());
        self.memos.push((key, memo));
    }

    fn memo(&self, key: u64) -> Option<&PaintMemo> {
        self.memo_index.get(&key).map(|&i| &self.memos[i].1)
    }

    pub(crate) fn new(dispatch_tree: DispatchTree) -> Self {
        Frame {
            focus: None,
            window_active: false,
            element_states: FxHashMap::default(),
            accessed_element_states: Vec::new(),
            mouse_listeners: Vec::new(),
            dispatch_tree,
            scene: Scene::default(),
            hitboxes: Vec::new(),
            window_control_hitboxes: Vec::new(),
            deferred_draws: Vec::new(),
            input_handlers: Vec::new(),
            tooltip_requests: Vec::new(),
            cursor_styles: Vec::new(),
            paint_records: Vec::new(),
            memos: Vec::new(),
            memo_index: FxHashMap::default(),

            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: FxHashMap::default(),

            #[cfg(any(feature = "inspector", debug_assertions))]
            next_inspector_instance_ids: FxHashMap::default(),

            #[cfg(any(feature = "inspector", debug_assertions))]
            inspector_hitboxes: FxHashMap::default(),
            tab_stops: TabStopMap::default(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.element_states.clear();
        self.accessed_element_states.clear();
        self.mouse_listeners.clear();
        self.dispatch_tree.clear();
        self.scene.clear();
        self.input_handlers.clear();
        self.tooltip_requests.clear();
        self.cursor_styles.clear();
        self.paint_records.clear();
        self.memos.clear();
        self.memo_index.clear();
        self.hitboxes.clear();
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.tab_stops.clear();
        self.focus = None;

        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            self.next_inspector_instance_ids.clear();
            self.inspector_hitboxes.clear();
        }
    }

    pub(crate) fn cursor_style(&self, window: &Window) -> Option<CursorStyle> {
        self.cursor_styles
            .iter()
            .rev()
            .fold_while(None, |style, request| match request.hitbox_id {
                None => Done(Some(request.style)),
                Some(hitbox_id) => Continue(
                    style.or_else(|| hitbox_id.is_hovered(window).then_some(request.style)),
                ),
            })
            .into_inner()
    }

    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> HitTest {
        let mut set_hover_hitbox_count = false;
        let mut hit_test = HitTest::default();
        for hitbox in self.hitboxes.iter().rev() {
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if bounds.contains(&position) {
                hit_test.ids.push(hitbox.id);
                if !set_hover_hitbox_count
                    && hitbox.behavior == HitboxBehavior::BlockMouseExceptScroll
                {
                    hit_test.hover_hitbox_count = hit_test.ids.len();
                    set_hover_hitbox_count = true;
                }
                if hitbox.behavior == HitboxBehavior::BlockMouse {
                    break;
                }
            }
        }
        if !set_hover_hitbox_count {
            hit_test.hover_hitbox_count = hit_test.ids.len();
        }
        hit_test
    }

    pub(crate) fn focus_path(&self) -> SmallVec<[FocusId; 8]> {
        self.focus
            .map(|focus_id| self.dispatch_tree.focus_path(focus_id))
            .unwrap_or_default()
    }

    pub(crate) fn finish(&mut self, prev_frame: &mut Self) {
        for element_state_key in &self.accessed_element_states {
            if let Some((element_state_key, element_state)) =
                prev_frame.element_states.remove_entry(element_state_key)
            {
                self.element_states.insert(element_state_key, element_state);
            }
        }

        self.scene.finish();
    }
}

/// Holds the state for a specific window.
pub struct Window {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) invalidator: WindowInvalidator,
    pub(crate) removed: bool,
    pub(crate) platform_window: Box<dyn PlatformWindow>,
    display_id: Option<DisplayId>,
    /// Olo: what kind it was opened as (a layer, a lock screen's).
    kind: crate::WindowKind,
    sprite_atlas: Arc<dyn PlatformAtlas>,
    /// Olo: the path masks drawn recently (`crate::outline`).
    outlines: crate::outline::Outlines,
    text_system: Arc<WindowTextSystem>,
    /// Olo: the faces and sizes an app asked to have drawn ahead
    /// ([`Window::warm_glyphs`]), and the scale and text rendering they
    /// were last drawn for (drawn again for another).
    warm: Vec<(FontId, Pixels)>,
    /// Olo: texts whose colour glyphs are drawn ahead ([`Window::warm_text`]),
    /// and the scale they were drawn for.
    warm_texts: Vec<(FontId, Pixels, Arc<[SharedString]>, f32)>,
    warmed_for: Option<(f32, crate::TextRendering)>,
    rem_size: Pixels,
    /// The stack of override values for the window's rem size.
    ///
    /// This is used by `with_rem_size` to allow rendering an element tree with
    /// a given rem size.
    rem_size_override_stack: SmallVec<[Pixels; 8]>,
    pub(crate) viewport_size: Size<Pixels>,
    layout_engine: Option<TaffyLayoutEngine>,
    pub(crate) root: Option<AnyView>,
    pub(crate) element_id_stack: SmallVec<[ElementId; 32]>,
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) requested_autoscroll: Option<Bounds<Pixels>>,
    pub(crate) image_cache_stack: Vec<AnyImageCache>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    next_hitbox_id: HitboxId,
    pub(crate) next_tooltip_id: TooltipId,
    pub(crate) tooltip_bounds: Option<TooltipBounds>,
    next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    pub(crate) dirty_views: FxHashSet<EntityId>,
    focus_listeners: SubscriberSet<(), AnyWindowFocusListener>,
    pub(crate) focus_lost_listeners: SubscriberSet<(), AnyObserver>,
    default_prevented: bool,
    mouse_position: Point<Pixels>,
    mouse_hit_test: HitTest,
    modifiers: Modifiers,
    capslock: Capslock,
    scale_factor: f32,
    pub(crate) bounds_observers: SubscriberSet<(), AnyObserver>,
    appearance: WindowAppearance,
    pub(crate) appearance_observers: SubscriberSet<(), AnyObserver>,
    active: Rc<Cell<bool>>,
    hovered: Rc<Cell<bool>>,
    pub(crate) needs_present: Rc<Cell<bool>>,
    /// Olo: this frame isn't to be shown (`skip_present`).
    skip_present: Cell<bool>,
    /// Olo: where the window's external pictures were placed, as last
    /// told the platform (`place_external`), and whether it ever had
    /// one (a window that hasn't looks for none).
    externals_placed: RefCell<Vec<(u64, Bounds<Pixels>)>>,
    had_externals: Cell<bool>,
    pub(crate) last_input_timestamp: Rc<Cell<Instant>>,
    pub(crate) frame_stats: FrameStats,
    /// Olo: something painted since the last paint memo began wasn't
    /// finished (`paint_unfinished`).
    paint_unfinished: bool,
    /// Olo: where the last draw's time went (`OLO_FRAME_PHASES`): its
    /// layout and prepaint, paint, and finishing the scene, each from
    /// the draw's start.
    phase_marks: [Duration; 3],
    /// Olo: the scroll views this frame drew ([`Window::note_scroll`]).
    frame_scrolls: Vec<[f32; 4]>,
    /// Olo: each glyph this frame painted, its raster bounds and atlas
    /// tile (None: nothing to draw), by face, glyph, size and subpixel
    /// place: a page of text is a few hundred glyphs again and again, and
    /// each looked up in the text system's and the atlas's maps (under
    /// their locks) cost more than the rest of painting it. Cleared each
    /// frame, so the atlas changing between frames can't leave a tile
    /// that's gone.
    glyph_memo:
        FxHashMap<(u32, u32, u32, u8, u8), Option<(Bounds<DevicePixels>, crate::AtlasTile)>>,
    /// Olo: what's dragged over the window now, where this frame's parts
    /// take it, where the platform was last told, and who takes drops.
    drop_offer: Option<crate::DropOffer>,
    drop_accept: Option<crate::DropAccept>,
    drop_accept_sent: Option<Option<crate::DropAccept>>,
    drop_handler: Option<Rc<RefCell<dyn FnMut(crate::Dropped, &mut Window, &mut App)>>>,
    /// Olo: the last draw was a refresh (`OLO_FRAME_PHASES`).
    last_draw_refreshed: bool,
    /// Olo: paint memos aren't to be replayed this draw (`App::refresh_windows`:
    /// something every window shows changed). A window's own `refresh`
    /// leaves them: their keys say all they depend on.
    pub(crate) memos_reset: bool,
    /// Olo: the last draw gave the platform the frame's last input
    /// handler (taken off the frame's list, put back as the next draw
    /// starts): its place is kept there whether or not the platform still
    /// has it.
    gave_input_handler: bool,
    pub(crate) refreshing: bool,
    pub(crate) activation_observers: SubscriberSet<(), AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    focus_enabled: bool,
    pending_input: Option<PendingInput>,
    pending_modifier: ModifierState,
    pub(crate) pending_input_observers: SubscriberSet<(), AnyObserver>,
    prompt: Option<RenderablePromptHandle>,
    pub(crate) client_inset: Option<Pixels>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    inspector: Option<Entity<Inspector>>,
}

#[derive(Clone, Debug, Default)]
struct ModifierState {
    modifiers: Modifiers,
    saw_keystroke: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawPhase {
    None,
    Prepaint,
    Paint,
    Focus,
}

#[derive(Default, Debug)]
struct PendingInput {
    keystrokes: SmallVec<[Keystroke; 1]>,
    focus: Option<FocusId>,
    timer: Option<Task<()>>,
}

pub(crate) struct ElementStateBox {
    pub(crate) inner: Box<dyn Any>,
    #[cfg(debug_assertions)]
    pub(crate) type_name: &'static str,
}

fn default_bounds(display_id: Option<DisplayId>, cx: &mut App) -> Bounds<Pixels> {
    const DEFAULT_WINDOW_OFFSET: Point<Pixels> = point(px(0.), px(35.));

    // TODO, BUG: if you open a window with the currently active window
    // on the stack, this will erroneously select the 'unwrap_or_else'
    // code path
    cx.active_window()
        .and_then(|w| w.update(cx, |_, window, _| window.bounds()).ok())
        .map(|mut bounds| {
            bounds.origin += DEFAULT_WINDOW_OFFSET;
            bounds
        })
        .unwrap_or_else(|| {
            let display = display_id
                .map(|id| cx.find_display(id))
                .unwrap_or_else(|| cx.primary_display());

            display
                .map(|display| display.default_bounds())
                .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE))
        })
}

impl Window {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        options: WindowOptions,
        cx: &mut App,
    ) -> Result<Self> {
        let WindowOptions {
            window_bounds,
            titlebar,
            focus,
            show,
            kind,
            is_movable,
            is_resizable,
            is_minimizable,
            display_id,
            window_background,
            app_id,
            window_min_size,
            window_max_size,
            window_decorations,
            #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
            tabbing_identifier,
            toplevel_tag,
            layer_shell,
            dialog,
            foreign_parent,
            session_lock_output,
        } = options;

        let bounds = window_bounds
            .map(|bounds| bounds.get_bounds())
            .unwrap_or_else(|| default_bounds(display_id, cx));
        // Olo: the title given (Wayland's platform didn't set it).
        let title = titlebar.as_ref().and_then(|t| t.title.clone());
        let mut platform_window = cx.platform.open_window(
            handle,
            WindowParams {
                bounds,
                titlebar,
                kind,
                is_movable,
                is_resizable,
                is_minimizable,
                focus,
                show,
                display_id,
                window_min_size,
                window_max_size,
                toplevel_tag,
                app_id: app_id.clone(),
                layer_shell,
                dialog,
                foreign_parent,
                session_lock_output,
                background: window_background,
                decorations: window_decorations.unwrap_or(WindowDecorations::Server),
                #[cfg(target_os = "macos")]
                tabbing_identifier,
            },
        )?;

        let tab_bar_visible = platform_window.tab_bar_visible();
        SystemWindowTabController::init_visible(cx, tab_bar_visible);
        if let Some(tabs) = platform_window.tabbed_windows() {
            SystemWindowTabController::add_tab(cx, handle.window_id(), tabs);
        }

        let display_id = platform_window.display().map(|display| display.id());
        let sprite_atlas = platform_window.sprite_atlas();
        let mouse_position = platform_window.mouse_position();
        let modifiers = platform_window.modifiers();
        let capslock = platform_window.capslock();
        let content_size = platform_window.content_size();
        let scale_factor = platform_window.scale_factor();
        let appearance = platform_window.appearance();
        let text_system = Arc::new(WindowTextSystem::new(cx.text_system().clone()));
        let invalidator = WindowInvalidator::new();
        invalidator.set_waker(platform_window.frame_waker());
        let active = Rc::new(Cell::new(platform_window.is_active()));
        let hovered = Rc::new(Cell::new(platform_window.is_hovered()));
        let needs_present = Rc::new(Cell::new(false));
        let next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>> = Default::default();
        let last_input_timestamp = Rc::new(Cell::new(Instant::now()));

        platform_window
            .request_decorations(window_decorations.unwrap_or(WindowDecorations::Server));
        platform_window.set_background_appearance(window_background);

        if let Some(ref window_open_state) = window_bounds {
            match window_open_state {
                WindowBounds::Fullscreen(_) => platform_window.toggle_fullscreen(),
                WindowBounds::Maximized(_) => platform_window.zoom(),
                WindowBounds::Windowed(_) => {}
            }
        }

        platform_window.on_close(Box::new({
            let window_id = handle.window_id();
            let mut cx = cx.to_async();
            move || {
                let _ = handle.update(&mut cx, |_, window, _| window.remove_window());
                let _ = cx.update(|cx| {
                    SystemWindowTabController::remove_tab(cx, window_id);
                });
            }
        }));
        // (Olo: the platform window's callbacks find the window gone
        // when the compositor's last events for it come after it closed:
        // nothing to do then, and nothing to say: `ok()`, not `log_err()`,
        // which logged "window not found" in pairs after windows closed.)
        platform_window.on_request_frame(Box::new({
            let mut cx = cx.to_async();
            let invalidator = invalidator.clone();
            let needs_present = needs_present.clone();
            let next_frame_callbacks = next_frame_callbacks.clone();
            move |request_frame_options| {
                let callbacks = next_frame_callbacks.take();
                if !callbacks.is_empty() {
                    handle
                        .update(&mut cx, |_, window, cx| {
                            for callback in callbacks {
                                callback(window, cx);
                            }
                        })
                        .ok();
                }

                // (Olo: no presenting the same scene again to keep a
                // display's refresh rate up: app_server doesn't lower it.)
                let needs_present =
                    request_frame_options.require_presentation || needs_present.get();

                let drew = if invalidator.is_dirty() || request_frame_options.force_render {
                    measure("frame duration", || {
                        handle
                            .update(&mut cx, |_, window, cx| -> bool {
                                let started = Instant::now();
                                let arena_clear_needed = window.draw(cx);
                                let drawn = started.elapsed();
                                // Olo: a frame drawn at a size about to
                                // change (a window that fits what it
                                // shows, measured in this very frame)
                                // isn't shown: the next, at its size, is.
                                // (Nothing is committed, so it says it
                                // drew nothing: no frame callback is
                                // waited for. The resize asks for the
                                // next frame.)
                                let skipped = window.skip_present.get();
                                if skipped {
                                    window.needs_present.set(true);
                                } else {
                                    window.present();
                                }
                                // drop the arena elements after present to reduce latency
                                arena_clear_needed.clear();
                                let scrolls = std::mem::take(&mut window.frame_scrolls);
                                window.frame_stats.record(started, drawn, started.elapsed(), scrolls);
                                if frame_phases() {
                                    let [layout, paint, finish] = window.phase_marks.map(|d| d.as_secs_f64() * 1000.0);
                                    let work = FRAME_WORK.take();
                                    let scene = &window.rendered_frame.scene;
                                    eprintln!(
                                        "frame {:.2} ms{}: layout {layout:.2}, paint {:.2}, finish {:.2}, present {:.2}; {} sprites, {} quads, {} operations; shaped {} lines in {:.2} ms, drew {} glyphs in {:.2} ms, found {} lines' glyphs in {:.2} ms",
                                        started.elapsed().as_secs_f64() * 1000.0,
                                        if window.last_draw_refreshed { " (refreshed)" } else { "" },
                                        paint - layout,
                                        finish - paint,
                                        (started.elapsed() - drawn).as_secs_f64() * 1000.0,
                                        scene.monochrome_sprites.len() + scene.polychrome_sprites.len(),
                                        scene.quads.len(),
                                        scene.paint_operations.len(),
                                        work[0].0,
                                        work[0].1,
                                        work[1].0,
                                        work[1].1,
                                        work[2].0,
                                        work[2].1,
                                    );
                                }
                                !skipped
                            })
                            .unwrap_or(false)
                    })
                } else if needs_present {
                    // (Unless the frame drawn last isn't to be shown:
                    // one drawn as the window opened, before a frame was
                    // asked for.)
                    handle
                        .update(&mut cx, |_, window, _| {
                            let skipped = window.skip_present.get();
                            if !skipped {
                                window.present();
                            }
                            !skipped
                        })
                        .unwrap_or(false)
                } else {
                    false
                };

                if drew {
                    handle
                        .update(&mut cx, |_, window, _| {
                            window.complete_frame();
                        })
                        .ok();
                }
                // Callbacks for the next frame keep the frames coming.
                if !next_frame_callbacks.borrow().is_empty() {
                    invalidator.wake();
                }
                drew
            }
        }));
        platform_window.on_resize(Box::new({
            let mut cx = cx.to_async();
            move |_, _| {
                handle
                    .update(&mut cx, |_, window, cx| window.bounds_changed(cx))
                    .ok();
            }
        }));
        platform_window.on_moved(Box::new({
            let mut cx = cx.to_async();
            move || {
                handle
                    .update(&mut cx, |_, window, cx| window.bounds_changed(cx))
                    .ok();
            }
        }));
        platform_window.on_appearance_changed(Box::new({
            let mut cx = cx.to_async();
            move || {
                handle
                    .update(&mut cx, |_, window, cx| window.appearance_changed(cx))
                    .ok();
            }
        }));
        platform_window.on_active_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                handle
                    .update(&mut cx, |_, window, cx| {
                        window.active.set(active);
                        window.modifiers = window.platform_window.modifiers();
                        window.capslock = window.platform_window.capslock();
                        window
                            .activation_observers
                            .clone()
                            .retain(&(), |callback| callback(window, cx));

                        window.bounds_changed(cx);
                        window.refresh();

                        SystemWindowTabController::update_last_active(cx, window.handle.id);
                    })
                    .ok();
            }
        }));
        platform_window.on_hover_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                handle
                    .update(&mut cx, |_, window, _| {
                        window.hovered.set(active);
                        window.refresh();
                    })
                    .ok();
            }
        }));
        platform_window.on_input({
            let mut cx = cx.to_async();
            Box::new(move |event| {
                handle
                    .update(&mut cx, |_, window, cx| window.dispatch_event(event, cx))
                    .ok()
                    .unwrap_or(DispatchEventResult::default())
            })
        });
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, window, _cx| {
                        for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                            if window.mouse_hit_test.ids.contains(&hitbox.id) {
                                return Some(*area);
                            }
                        }
                        None
                    })
                    .ok()
                    .unwrap_or(None)
            })
        });
        platform_window.on_move_tab_to_new_window({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, _window, cx| {
                        SystemWindowTabController::move_tab_to_new_window(cx, handle.window_id());
                    })
                    .ok();
            })
        });
        platform_window.on_merge_all_windows({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, _window, cx| {
                        SystemWindowTabController::merge_all_windows(cx, handle.window_id());
                    })
                    .ok();
            })
        });
        platform_window.on_select_next_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, _window, cx| {
                        SystemWindowTabController::select_next_tab(cx, handle.window_id());
                    })
                    .ok();
            })
        });
        platform_window.on_select_previous_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, _window, cx| {
                        SystemWindowTabController::select_previous_tab(cx, handle.window_id())
                    })
                    .ok();
            })
        });
        platform_window.on_toggle_tab_bar({
            let mut cx = cx.to_async();
            Box::new(move || {
                handle
                    .update(&mut cx, |_, window, cx| {
                        let tab_bar_visible = window.platform_window.tab_bar_visible();
                        SystemWindowTabController::set_visible(cx, tab_bar_visible);
                    })
                    .ok();
            })
        });

        if let Some(app_id) = app_id {
            platform_window.set_app_id(&app_id);
        }
        if let Some(title) = title {
            platform_window.set_title(&title);
        }

        platform_window.map_window().unwrap();

        Ok(Window {
            handle,
            invalidator,
            removed: false,
            platform_window,
            display_id,
            kind,
            sprite_atlas,
            outlines: Default::default(),
            text_system,
            warm: Vec::new(),
            warm_texts: Vec::new(),
            warmed_for: None,
            rem_size: px(16.),
            rem_size_override_stack: SmallVec::new(),
            viewport_size: content_size,
            layout_engine: Some(TaffyLayoutEngine::new()),
            root: None,
            element_id_stack: SmallVec::default(),
            text_style_stack: Vec::new(),
            rendered_entity_stack: Vec::new(),
            element_offset_stack: Vec::new(),
            content_mask_stack: Vec::new(),
            element_opacity: 1.0,
            requested_autoscroll: None,
            rendered_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            next_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            next_frame_callbacks,
            next_hitbox_id: HitboxId(0),
            next_tooltip_id: TooltipId::default(),
            tooltip_bounds: None,
            dirty_views: FxHashSet::default(),
            focus_listeners: SubscriberSet::new(),
            focus_lost_listeners: SubscriberSet::new(),
            default_prevented: true,
            mouse_position,
            mouse_hit_test: HitTest::default(),
            modifiers,
            capslock,
            scale_factor,
            bounds_observers: SubscriberSet::new(),
            appearance,
            appearance_observers: SubscriberSet::new(),
            active,
            hovered,
            needs_present,
            skip_present: Cell::new(false),
            externals_placed: RefCell::default(),
            had_externals: Cell::new(false),
            last_input_timestamp,
            frame_stats: FrameStats::default(),
            paint_unfinished: false,
            phase_marks: Default::default(),
            frame_scrolls: Vec::new(),
            glyph_memo: FxHashMap::default(),
            drop_offer: None,
            drop_accept: None,
            drop_accept_sent: None,
            drop_handler: None,
            refreshing: false,
            last_draw_refreshed: false,
            memos_reset: false,
            gave_input_handler: false,
            activation_observers: SubscriberSet::new(),
            focus: None,
            focus_enabled: true,
            pending_input: None,
            pending_modifier: ModifierState::default(),
            pending_input_observers: SubscriberSet::new(),
            prompt: None,
            client_inset: None,
            image_cache_stack: Vec::new(),
            #[cfg(any(feature = "inspector", debug_assertions))]
            inspector: None,
        })
    }

    pub(crate) fn new_focus_listener(
        &self,
        value: AnyWindowFocusListener,
    ) -> (Subscription, impl FnOnce() + use<>) {
        self.focus_listeners.insert((), value)
    }
}

/// What dispatching an event came to (`Window::dispatch_event`). (Olo:
/// public, as the dispatch is: the kit clicks a tab for `spotlight`.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DispatchEventResult {
    /// Nothing stopped it.
    pub propagate: bool,
    /// Something said its default isn't to happen.
    pub default_prevented: bool,
}

/// Indicates which region of the window is visible. Content falling outside of this mask will not be
/// rendered. Currently, only rectangular content masks are supported, but we give the mask its own type
/// to leave room to support more complex shapes in the future.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    /// The bounds
    pub bounds: Bounds<P>,
}

impl ContentMask<Pixels> {
    /// Scale the content mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.bounds.scale(factor),
        }
    }

    /// Intersect the content mask with the given content mask.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        ContentMask { bounds }
    }
}

impl Window {
    fn mark_view_dirty(&mut self, view_id: EntityId) {
        // Mark ancestor views as dirty. If already in the `dirty_views` set, then all its ancestors
        // should already be dirty.
        for view_id in self
            .rendered_frame
            .dispatch_tree
            .view_path(view_id)
            .into_iter()
            .rev()
        {
            if !self.dirty_views.insert(view_id) {
                break;
            }
        }
    }

    /// Registers a callback to be invoked when the window appearance changes.
    pub fn observe_window_appearance(
        &self,
        mut callback: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.appearance_observers.insert(
            (),
            Box::new(move |window, cx| {
                callback(window, cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Replaces the root entity of the window with a new one.
    pub fn replace_root<E>(
        &mut self,
        cx: &mut App,
        build_view: impl FnOnce(&mut Window, &mut Context<E>) -> E,
    ) -> Entity<E>
    where
        E: 'static + Render,
    {
        let view = cx.new(|cx| build_view(self, cx));
        self.root = Some(view.clone().into());
        self.refresh();
        view
    }

    /// Returns the root entity of the window, if it has one.
    pub fn root<E>(&self) -> Option<Option<Entity<E>>>
    where
        E: 'static + Render,
    {
        self.root
            .as_ref()
            .map(|view| view.clone().downcast::<E>().ok())
    }

    /// Olo: a scroll view drawn this frame shows its content from (`x`,
    /// `y`) and goes as far as (`max_x`, `max_y`), for the frame's mark
    /// (how evenly it scrolls, measured: tools/dev/scroll-check).
    pub fn note_scroll(&mut self, at: [f32; 4]) {
        self.frame_scrolls.push(at);
    }

    /// Olo: how the window kept up since this was last called (and
    /// starts counting again).
    pub fn take_frame_stats(&mut self) -> FrameStats {
        std::mem::take(&mut self.frame_stats)
    }

    /// Obtain a handle to the window that belongs to this context.
    pub fn window_handle(&self) -> AnyWindowHandle {
        self.handle
    }

    /// Olo: don't show the frame being drawn (call it from paint): its
    /// window is about to take another size (resize it, deferred), or
    /// isn't to be shown yet. Nothing is committed: whatever changes
    /// next (the resize) draws the next frame, which is shown. A window
    /// not shown yet stays unshown until then (Wayland shows a surface
    /// with its first buffer).
    pub fn skip_present(&self) {
        self.skip_present.set(true);
    }

    /// Mark the window as dirty, scheduling it to be redrawn on the next frame.
    pub fn refresh(&mut self) {
        if self.invalidator.not_drawing() {
            self.refreshing = true;
            self.invalidator.set_dirty(true);
        }
    }

    /// Olo: draws the whole window again now, even out of sight (where
    /// the compositor holds back its frame callbacks): for a frame wanted
    /// for itself, a layout review's.
    pub fn draw_now(&mut self) {
        self.refresh();
        // (All of it painted again, nothing replayed: what's painted is
        // what's being looked at.)
        self.memos_reset = true;
        self.platform_window.draw_now();
    }

    /// Close this window.
    pub fn remove_window(&mut self) {
        self.removed = true;
    }

    /// Olo: ask this window to close as its close button does: its
    /// should-close hook first ([`Window::on_window_should_close`]: an
    /// app's confirmations), then closed if it agrees. Done on the event
    /// loop's next turn, so the hook can update the window.
    pub fn request_close(&self) {
        self.platform_window.request_close();
    }

    /// Obtain the currently focused [`FocusHandle`]. If no elements are focused, returns `None`.
    pub fn focused(&self, cx: &App) -> Option<FocusHandle> {
        self.focus
            .and_then(|id| FocusHandle::for_id(id, &cx.focus_handles))
    }

    /// Move focus to the element associated with the given [`FocusHandle`].
    pub fn focus(&mut self, handle: &FocusHandle) {
        if !self.focus_enabled || self.focus == Some(handle.id) {
            return;
        }

        self.focus = Some(handle.id);
        self.clear_pending_keystrokes();
        self.refresh();
    }

    /// Remove focus from all elements within this context's window.
    pub fn blur(&mut self) {
        if !self.focus_enabled {
            return;
        }

        self.focus = None;
        self.refresh();
    }

    /// Blur the window and don't allow anything in it to be focused again.
    pub fn disable_focus(&mut self) {
        self.blur();
        self.focus_enabled = false;
    }

    /// Move focus to next tab stop.
    pub fn focus_next(&mut self) {
        if !self.focus_enabled {
            return;
        }

        if let Some(handle) = self.rendered_frame.tab_stops.next(self.focus.as_ref()) {
            self.focus(&handle)
        }
    }

    /// Move focus to previous tab stop.
    pub fn focus_prev(&mut self) {
        if !self.focus_enabled {
            return;
        }

        if let Some(handle) = self.rendered_frame.tab_stops.prev(self.focus.as_ref()) {
            self.focus(&handle)
        }
    }

    /// Accessor for the text system.
    pub fn text_system(&self) -> &Arc<WindowTextSystem> {
        &self.text_system
    }

    /// The current text style. Which is composed of all the style refinements provided to `with_text_style`.
    pub fn text_style(&self) -> TextStyle {
        let mut style = TextStyle::default();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        style
    }

    /// Check if the platform window is maximized
    /// On some platforms (namely Windows) this is different than the bounds being the size of the display
    pub fn is_maximized(&self) -> bool {
        self.platform_window.is_maximized()
    }

    /// request a certain window decoration (Wayland)
    pub fn request_decorations(&self, decorations: WindowDecorations) {
        self.platform_window.request_decorations(decorations);
    }

    /// Start a window resize operation (Wayland)
    pub fn start_window_resize(&self, edge: ResizeEdge) {
        self.platform_window.start_window_resize(edge);
    }

    /// Return the `WindowBounds` to indicate that how a window should be opened
    /// after it has been closed
    pub fn window_bounds(&self) -> WindowBounds {
        self.platform_window.window_bounds()
    }

    /// Return the `WindowBounds` excluding insets (Wayland and X11)
    pub fn inner_window_bounds(&self) -> WindowBounds {
        self.platform_window.inner_window_bounds()
    }

    /// Dispatch the given action on the currently focused element.
    pub fn dispatch_action(&mut self, action: Box<dyn Action>, cx: &mut App) {
        let focus_id = self.focused(cx).map(|handle| handle.id);

        let window = self.handle;
        cx.defer(move |cx| {
            window
                .update(cx, |_, window, cx| {
                    let node_id = window.focus_node_id_in_rendered_frame(focus_id);
                    window.dispatch_action_on_node(node_id, action.as_ref(), cx);
                })
                .ok();
        })
    }

    pub(crate) fn dispatch_keystroke_observers(
        &mut self,
        event: &dyn Any,
        action: Option<Box<dyn Action>>,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() else {
            return;
        };

        cx.keystroke_observers.clone().retain(&(), move |callback| {
            (callback)(
                &KeystrokeEvent {
                    keystroke: key_down_event.keystroke.clone(),
                    action: action.as_ref().map(|action| action.boxed_clone()),
                    context_stack: context_stack.clone(),
                },
                self,
                cx,
            )
        });
    }

    pub(crate) fn dispatch_keystroke_interceptors(
        &mut self,
        event: &dyn Any,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() else {
            return;
        };

        cx.keystroke_interceptors
            .clone()
            .retain(&(), move |callback| {
                (callback)(
                    &KeystrokeEvent {
                        keystroke: key_down_event.keystroke.clone(),
                        action: None,
                        context_stack: context_stack.clone(),
                    },
                    self,
                    cx,
                )
            });
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let handle = self.handle;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| f(window, cx)).ok();
        });
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn observe<T: 'static>(
        &mut self,
        observed: &Entity<T>,
        cx: &mut App,
        mut on_notify: impl FnMut(Entity<T>, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let entity_id = observed.entity_id();
        let observed = observed.downgrade();
        let window_handle = self.handle;
        cx.new_observer(
            entity_id,
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| {
                        if let Some(handle) = observed.upgrade() {
                            on_notify(handle, window, cx);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
            }),
        )
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn subscribe<Emitter, Evt>(
        &mut self,
        entity: &Entity<Emitter>,
        cx: &mut App,
        mut on_event: impl FnMut(Entity<Emitter>, &Evt, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        Emitter: EventEmitter<Evt>,
        Evt: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        let window_handle = self.handle;
        cx.new_subscription(
            entity_id,
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    window_handle
                        .update(cx, |_, window, cx| {
                            if let Some(entity) = handle.upgrade() {
                                let event = event.downcast_ref().expect("invalid event type");
                                on_event(entity, event, window, cx);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                }),
            ),
        )
    }

    /// Register a callback to be invoked when the given `Entity` is released.
    pub fn observe_release<T>(
        &self,
        entity: &Entity<T>,
        cx: &mut App,
        mut on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let entity_id = entity.entity_id();
        let window_handle = self.handle;
        let (subscription, activate) = cx.release_listeners.insert(
            entity_id,
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                let _ = window_handle.update(cx, |_, window, cx| on_release(entity, window, cx));
            }),
        );
        activate();
        subscription
    }

    /// Creates an [`AsyncWindowContext`], which has a static lifetime and can be held across
    /// await points in async code.
    pub fn to_async(&self, cx: &App) -> AsyncWindowContext {
        AsyncWindowContext::new_context(cx.to_async(), self.handle)
    }

    /// Schedule the given closure to be run directly after the current frame is rendered.
    pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(callback));
        self.invalidator.wake();
    }

    /// Schedule a frame to be drawn on the next animation frame.
    ///
    /// This is useful for elements that need to animate continuously, such as a video player or an animated GIF.
    /// It will cause the window to redraw on the next frame, even if no other changes have occurred.
    ///
    /// If called from within a view, it will notify that view on the next frame. Otherwise, it will refresh the entire window.
    pub fn request_animation_frame(&self) {
        // (Outside drawing, an event handler's, there is no current view:
        // `current_view` would panic. The whole window, as said above;
        // Tracker's band selection crashed on it.)
        match self.rendered_entity_stack.last().copied() {
            Some(entity) => self.on_next_frame(move |_, cx| cx.notify(entity)),
            None => self.on_next_frame(|window, _| window.refresh()),
        }
    }

    /// Spawn the future returned by the given closure on the application thread pool.
    /// The closure is provided a handle to the current window and an `AsyncWindowContext` for
    /// use within your future.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    fn bounds_changed(&mut self, cx: &mut App) {
        self.scale_factor = self.platform_window.scale_factor();
        self.viewport_size = self.platform_window.content_size();
        self.display_id = self.platform_window.display().map(|display| display.id());

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    /// Returns the bounds of the current window in the global coordinate space, which could span across multiple displays.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    /// Set the content size of the window.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform_window.resize(size);
    }

    /// Returns whether or not the window is currently fullscreen
    pub fn is_fullscreen(&self) -> bool {
        self.platform_window.is_fullscreen()
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        self.appearance = self.platform_window.appearance();

        self.appearance_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    /// Returns the appearance of the current window.
    pub fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    /// Returns the size of the drawable area within the window.
    pub fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    /// Returns whether this window is focused by the operating system (receiving key events).
    pub fn is_window_active(&self) -> bool {
        self.active.get()
    }

    /// Returns whether this window is considered to be the window
    /// that currently owns the mouse cursor.
    /// On mac, this is equivalent to `is_window_active`.
    pub fn is_window_hovered(&self) -> bool {
        if cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )) {
            self.hovered.get()
        } else {
            self.is_window_active()
        }
    }

    /// Toggle zoom on the window.
    pub fn zoom_window(&self) {
        self.platform_window.zoom();
    }

    /// Opens the native title bar context menu, useful when implementing client side decorations (Wayland and X11)
    pub fn show_window_menu(&self, position: Point<Pixels>) {
        self.platform_window.show_window_menu(position)
    }

    /// Tells the compositor to take control of window movement (Wayland and X11)
    ///
    /// Events may not be received during a move operation.
    pub fn start_window_move(&self) {
        self.platform_window.start_window_move()
    }

    /// Olo: drags `drag` out of the window to another app or a folder
    /// (call it while the pointer's button is held, as a drag begins):
    /// what it was dropped as comes when it's over (None: cancelled, or
    /// it couldn't start).
    pub fn start_drag(
        &self,
        drag: crate::OutgoingDrag,
    ) -> futures::channel::oneshot::Receiver<Option<crate::DragAction>> {
        let (done, over) = futures::channel::oneshot::channel();
        self.platform_window.start_drag(drag, done);
        over
    }

    /// Olo: what kind of window it was opened as.
    pub fn window_kind(&self) -> crate::WindowKind {
        self.kind
    }

    /// Olo: the window's surface's protocol id (Wayland's `wl_surface`),
    /// which app_server names it by (its keyboard hints ask about it).
    pub fn surface_id(&self) -> Option<u32> {
        self.platform_window.surface_id()
    }

    /// When using client side decorations, set this to the width of the invisible decorations (Wayland and X11)
    pub fn set_client_inset(&mut self, inset: Pixels) {
        self.client_inset = Some(inset);
        self.platform_window.set_client_inset(inset);
    }

    /// Returns the client_inset value by [`Self::set_client_inset`].
    pub fn client_inset(&self) -> Option<Pixels> {
        self.client_inset
    }

    /// Olo: which sides the compositor holds (in a tile, maximized or
    /// full screen), whoever draws the decorations: a size it chose, not
    /// the person.
    pub fn tiling(&self) -> crate::Tiling {
        self.platform_window.tiling()
    }

    /// Returns whether the title bar window controls need to be rendered by the application (Wayland and X11)
    pub fn window_decorations(&self) -> Decorations {
        self.platform_window.window_decorations()
    }

    /// Returns which window controls are currently visible (Wayland)
    pub fn window_controls(&self) -> WindowControls {
        self.platform_window.window_controls()
    }

    /// Updates the window's title at the platform level.
    pub fn set_window_title(&mut self, title: &str) {
        self.platform_window.set_title(title);
    }

    /// Olo: whether the window is on a display whose HDR is on: an HDR
    /// picture may then be drawn as HDR (`set_gamut(Gamut::Hdr10)`).
    pub fn on_hdr_display(&self) -> bool {
        self.platform_window.on_hdr_display()
    }

    /// Olo: draw this window in `space` (Display P3, while it shows a
    /// photo wider than sRGB, say; HDR10, an HDR video): its sRGB colours
    /// converted as drawn, images in that space as they are. Whether it
    /// does: the compositor must take the window's colours
    /// (wp_color_management_v1).
    pub fn set_gamut(&mut self, space: crate::Gamut) -> bool {
        let drawn = self.platform_window.set_gamut(space);
        self.refresh();
        drawn
    }

    /// Sets the application identifier.
    pub fn set_app_id(&mut self, app_id: &str) {
        self.platform_window.set_app_id(app_id);
    }

    /// Sets the window background appearance.
    pub fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.platform_window
            .set_background_appearance(background_appearance);
    }

    /// Mark the window as dirty at the platform level.
    pub fn set_window_edited(&mut self, edited: bool) {
        self.platform_window.set_edited(edited);
    }

    /// Determine the display on which the window is visible.
    pub fn display(&self, cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
        cx.platform
            .displays()
            .into_iter()
            .find(|display| Some(display.id()) == self.display_id)
    }

    /// Show the platform character palette.
    pub fn show_character_palette(&self) {
        self.platform_window.show_character_palette();
    }

    /// The scale factor of the display associated with the window. For example, it could
    /// return 2.0 for a "retina" display, indicating that each logical pixel should actually
    /// be rendered as two pixels on screen.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// The size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn rem_size(&self) -> Pixels {
        self.rem_size_override_stack
            .last()
            .copied()
            .unwrap_or(self.rem_size)
    }

    /// Sets the size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn set_rem_size(&mut self, rem_size: impl Into<Pixels>) {
        self.rem_size = rem_size.into();
    }

    /// Acquire a globally unique identifier for the given ElementId.
    /// Only valid for the duration of the provided closure.
    pub fn with_global_id<R>(
        &mut self,
        element_id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id);
        let global_id = GlobalElementId(self.element_id_stack.clone());
        let result = f(&global_id, self);
        self.element_id_stack.pop();
        result
    }

    /// Executes the provided function with the specified rem size.
    ///
    /// This method must only be called as part of element drawing.
    pub fn with_rem_size<F, R>(&mut self, rem_size: Option<impl Into<Pixels>>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(rem_size) = rem_size {
            self.rem_size_override_stack.push(rem_size.into());
            let result = f(self);
            self.rem_size_override_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// The line height associated with the current text style.
    pub fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels(self.rem_size())
    }

    /// Call to prevent the default action of an event. Currently only used to prevent
    /// parent elements from becoming focused on mouse down.
    pub fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    /// Obtain whether default has been prevented for the event currently being dispatched.
    pub fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    /// Determine whether the given action is available along the dispatch path to the currently focused element.
    pub fn is_action_available(&self, action: &dyn Action, cx: &mut App) -> bool {
        let node_id =
            self.focus_node_id_in_rendered_frame(self.focused(cx).map(|handle| handle.id));
        self.rendered_frame
            .dispatch_tree
            .is_action_available(action, node_id)
    }

    /// The position of the mouse relative to the window.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position
    }

    /// The current state of the keyboard's modifiers
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// The current state of the keyboard's capslock
    pub fn capslock(&self) -> Capslock {
        self.capslock
    }

    fn complete_frame(&self) {
        self.platform_window.completed_frame();
    }

    /// Olo: draws what changed since the last frame, without showing it
    /// (the next frame does). A window that isn't showing (on another
    /// workspace, minimized, covered) draws nothing until it shows, so
    /// what an agent reads of it (its tree, its menus) would be as it was
    /// last drawn, not as it is.
    pub fn draw_if_dirty(&mut self, cx: &mut App) {
        if self.invalidator.is_dirty() {
            self.draw(cx).clear();
        }
    }

    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], use [`Self::present`].
    #[profiling::function]
    pub fn draw(&mut self, cx: &mut App) -> ArenaClearNeeded {
        // (Olo: whether this frame is shown is this frame's to say.)
        self.skip_present.set(false);
        self.frame_scrolls.clear();
        self.glyph_memo.clear();
        self.drop_accept = None;
        if !self.warm.is_empty() || !self.warm_texts.is_empty() {
            self.warm_for_display();
        }
        self.invalidate_entities();
        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        self.invalidator.set_dirty(false);
        self.requested_autoscroll = None;

        // Restore the previously-used input handler. (Olo: or keep its
        // place, if the platform hasn't it now: a draw while an event has
        // it out (`dispatch_input`). The frame's paint indices count it,
        // and replaying a memo painted after it sliced past the list's
        // end: Notifications' History panicked as it opened.)
        match self.platform_window.take_input_handler() {
            Some(input_handler) => self.rendered_frame.input_handlers.push(Some(input_handler)),
            None if self.gave_input_handler => self.rendered_frame.input_handlers.push(None),
            None => {}
        }
        let started = Instant::now();
        self.draw_roots(cx, started);
        self.phase_marks[1] = started.elapsed();
        self.outlines.end_frame(self.sprite_atlas.as_ref());
        // Olo: where a window that takes drops took what's dragged over
        // it, told the platform as it changes (one that doesn't, keeps
        // the platform's own: files, copied).
        if self.drop_handler.is_some()
            && self.drop_offer.is_some()
            && self.drop_accept_sent.as_ref() != Some(&self.drop_accept)
        {
            self.platform_window
                .set_drop_accept(self.drop_accept.clone());
            self.drop_accept_sent = Some(self.drop_accept.clone());
        }
        self.dirty_views.clear();
        self.next_frame.window_active = self.active.get();

        // Register requested input handler with the platform window.
        // (Olo: one replayed from a place kept empty has none to give.)
        self.gave_input_handler = false;
        if let Some(input_handler) = self.next_frame.input_handlers.pop() {
            self.gave_input_handler = true;
            if let Some(input_handler) = input_handler {
                self.platform_window.set_input_handler(input_handler);
            }
        }

        self.layout_engine.as_mut().unwrap().clear();
        self.text_system().finish_frame();
        self.next_frame.finish(&mut self.rendered_frame);
        self.phase_marks[2] = started.elapsed();

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;

        if previous_focus_path != current_focus_path
            || previous_window_active != current_window_active
        {
            if !previous_focus_path.is_empty() && current_focus_path.is_empty() {
                self.focus_lost_listeners
                    .clone()
                    .retain(&(), |listener| listener(self, cx));
            }

            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active {
                    previous_focus_path
                } else {
                    Default::default()
                },
                current_focus_path: if current_window_active {
                    current_focus_path
                } else {
                    Default::default()
                },
            };
            self.focus_listeners
                .clone()
                .retain(&(), |listener| listener(&event, self, cx));
        }

        debug_assert!(self.rendered_entity_stack.is_empty());
        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.last_draw_refreshed = std::mem::replace(&mut self.refreshing, false);
        self.memos_reset = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.needs_present.set(true);

        ArenaClearNeeded
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        let mut entities = mem::take(entities_ref.deref_mut());
        drop(entities_ref);
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        mem::swap(&mut entities, entities_ref.deref_mut());
    }

    fn invalidate_entities(&mut self) {
        let mut views = self.invalidator.take_views();
        for entity in views.drain() {
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
    }

    #[profiling::function]
    fn present(&self) {
        if self.had_externals.get() {
            // Olo: the external pictures where this frame placed them
            // (replayed with a cached view's paint, moved with it).
            let places: Vec<(u64, Bounds<Pixels>)> = self
                .rendered_frame
                .paint_records
                .iter()
                .filter_map(|(r, moved)| {
                    r.downcast_ref::<ExternalPlace>().map(|p| {
                        (
                            p.key,
                            Bounds {
                                origin: p.bounds.origin + *moved,
                                size: p.bounds.size,
                            },
                        )
                    })
                })
                .collect();
            if *self.externals_placed.borrow() != places {
                self.platform_window.place_externals(&places);
                *self.externals_placed.borrow_mut() = places;
            }
        }
        self.platform_window.draw(&self.rendered_frame.scene);
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    fn draw_roots(&mut self, cx: &mut App, started: Instant) {
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();

        let _inspector_width: Pixels = rems(30.0).to_pixels(self.rem_size());
        let root_size = {
            #[cfg(any(feature = "inspector", debug_assertions))]
            {
                if self.inspector.is_some() {
                    let mut size = self.viewport_size;
                    size.width = (size.width - _inspector_width).max(px(0.0));
                    size
                } else {
                    self.viewport_size
                }
            }
            #[cfg(not(any(feature = "inspector", debug_assertions)))]
            {
                self.viewport_size
            }
        };

        // Layout all root elements.
        let mut root_element = self.root.as_ref().unwrap().clone().into_any();
        root_element.prepaint_as_root(Point::default(), root_size.into(), self, cx);

        #[cfg(any(feature = "inspector", debug_assertions))]
        let inspector_element = self.prepaint_inspector(_inspector_width, cx);

        let mut sorted_deferred_draws =
            (0..self.next_frame.deferred_draws.len()).collect::<SmallVec<[_; 8]>>();
        sorted_deferred_draws.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        self.prepaint_deferred_draws(&sorted_deferred_draws, cx);

        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any();
            element.prepaint_as_root(Point::default(), root_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

        // Now actually paint the elements.
        self.invalidator.set_phase(DrawPhase::Paint);
        self.phase_marks[0] = started.elapsed();
        root_element.paint(self, cx);

        #[cfg(any(feature = "inspector", debug_assertions))]
        self.paint_inspector(inspector_element, cx);

        self.paint_deferred_draws(&sorted_deferred_draws, cx);

        if let Some(mut prompt_element) = prompt_element {
            prompt_element.paint(self, cx);
        } else if let Some(mut drag_element) = active_drag_element {
            drag_element.paint(self, cx);
        } else if let Some(mut tooltip_element) = tooltip_element {
            tooltip_element.paint(self, cx);
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        self.paint_inspector_hitbox(cx);
    }

    fn prepaint_tooltip(&mut self, cx: &mut App) -> Option<AnyElement> {
        // Use indexing instead of iteration to avoid borrowing self for the duration of the loop.
        for tooltip_request_index in (0..self.next_frame.tooltip_requests.len()).rev() {
            let Some(Some(tooltip_request)) = self
                .next_frame
                .tooltip_requests
                .get(tooltip_request_index)
                .cloned()
            else {
                log::error!("Unexpectedly absent TooltipRequest");
                continue;
            };
            let mut element = tooltip_request.tooltip.view.clone().into_any();
            let mouse_position = tooltip_request.tooltip.mouse_position;
            let tooltip_size = element.layout_as_root(AvailableSpace::min_size(), self, cx);

            let mut tooltip_bounds =
                Bounds::new(mouse_position + point(px(1.), px(1.)), tooltip_size);
            let window_bounds = Bounds {
                origin: Point::default(),
                size: self.viewport_size(),
            };

            if tooltip_bounds.right() > window_bounds.right() {
                let new_x = mouse_position.x - tooltip_bounds.size.width - px(1.);
                if new_x >= Pixels::ZERO {
                    tooltip_bounds.origin.x = new_x;
                } else {
                    tooltip_bounds.origin.x = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.x - tooltip_bounds.right() - window_bounds.right(),
                    );
                }
            }

            if tooltip_bounds.bottom() > window_bounds.bottom() {
                let new_y = mouse_position.y - tooltip_bounds.size.height - px(1.);
                if new_y >= Pixels::ZERO {
                    tooltip_bounds.origin.y = new_y;
                } else {
                    tooltip_bounds.origin.y = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.y - tooltip_bounds.bottom() - window_bounds.bottom(),
                    );
                }
            }

            // It's possible for an element to have an active tooltip while not being painted (e.g.
            // via the `visible_on_hover` method). Since mouse listeners are not active in this
            // case, instead update the tooltip's visibility here.
            let is_visible =
                (tooltip_request.tooltip.check_visible_and_update)(tooltip_bounds, self, cx);
            if !is_visible {
                continue;
            }

            self.with_absolute_element_offset(tooltip_bounds.origin, |window| {
                element.prepaint(window, cx)
            });

            self.tooltip_bounds = Some(TooltipBounds {
                id: tooltip_request.id,
                bounds: tooltip_bounds,
            });
            return Some(element);
        }
        None
    }

    fn prepaint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.text_style_stack
                .clone_from(&deferred_draw.text_style_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let prepaint_start = self.prepaint_index();
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(deferred_draw.current_view, |window| {
                    window.with_absolute_element_offset(deferred_draw.absolute_offset, |window| {
                        element.prepaint(window, cx)
                    });
                })
            } else {
                self.reuse_prepaint(deferred_draw.prepaint_range.clone());
            }
            let prepaint_end = self.prepaint_index();
            deferred_draw.prepaint_range = prepaint_start..prepaint_end;
        }
        assert_eq!(
            self.next_frame.deferred_draws.len(),
            0,
            "cannot call defer_draw during deferred drawing"
        );
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
        self.text_style_stack.clear();
    }

    fn paint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            let mut deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let paint_start = self.paint_index();
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(deferred_draw.current_view, |window| {
                    element.paint(window, cx);
                })
            } else {
                self.reuse_paint(deferred_draw.paint_range.clone());
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
    }

    pub(crate) fn prepaint_index(&self) -> PrepaintStateIndex {
        PrepaintStateIndex {
            hitboxes_index: self.next_frame.hitboxes.len(),
            tooltips_index: self.next_frame.tooltip_requests.len(),
            deferred_draws_index: self.next_frame.deferred_draws.len(),
            dispatch_tree_index: self.next_frame.dispatch_tree.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn reuse_prepaint(&mut self, range: Range<PrepaintStateIndex>) {
        self.next_frame.hitboxes.extend(
            self.rendered_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index]
                .iter()
                .cloned(),
        );
        self.next_frame.tooltip_requests.extend(
            self.rendered_frame.tooltip_requests
                [range.start.tooltips_index..range.end.tooltips_index]
                .iter_mut()
                .map(|request| request.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);

        let reused_subtree = self.next_frame.dispatch_tree.reuse_subtree(
            range.start.dispatch_tree_index..range.end.dispatch_tree_index,
            &mut self.rendered_frame.dispatch_tree,
            self.focus,
        );

        if reused_subtree.contains_focus() {
            self.next_frame.focus = self.focus;
        }

        self.next_frame.deferred_draws.extend(
            self.rendered_frame.deferred_draws
                [range.start.deferred_draws_index..range.end.deferred_draws_index]
                .iter()
                .map(|deferred_draw| DeferredDraw {
                    current_view: deferred_draw.current_view,
                    parent_node: reused_subtree.refresh_node_id(deferred_draw.parent_node),
                    element_id_stack: deferred_draw.element_id_stack.clone(),
                    text_style_stack: deferred_draw.text_style_stack.clone(),
                    priority: deferred_draw.priority,
                    element: None,
                    absolute_offset: deferred_draw.absolute_offset,
                    prepaint_range: deferred_draw.prepaint_range.clone(),
                    paint_range: deferred_draw.paint_range.clone(),
                }),
        );
    }

    pub(crate) fn paint_index(&self) -> PaintIndex {
        PaintIndex {
            scene_index: self.next_frame.scene.len(),
            mouse_listeners_index: self.next_frame.mouse_listeners.len(),
            input_handlers_index: self.next_frame.input_handlers.len(),
            cursor_styles_index: self.next_frame.cursor_styles.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            tab_handle_index: self.next_frame.tab_stops.paint_index(),
            line_layout_index: self.text_system.layout_index(),
            paint_records_index: self.next_frame.paint_records.len(),
            memos_index: self.next_frame.memos.len(),
        }
    }

    /// Olo: notes `record` as part of what's being painted (what a widget
    /// is, for agents): kept with the frame, and replayed with a cached
    /// view's paint as its scene is. Read the last frame's with
    /// [`Window::paint_records`].
    pub fn push_paint_record(&mut self, record: Rc<dyn Any>) {
        self.next_frame
            .paint_records
            .push((record, Point::default()));
    }

    /// Olo: what was noted as the last frame painted
    /// ([`Window::push_paint_record`]), in painting order, each with how
    /// far it was moved since it was noted (add it to where it says it
    /// is: [`Window::paint_memo`] replays moved).
    pub fn paint_records(&self) -> &[(Rc<dyn Any>, Point<Pixels>)] {
        &self.rendered_frame.paint_records
    }

    /// Olo: what the frame being painted has noted so far, as
    /// [`Window::paint_records`] has the last one's: for what paints last
    /// to read all of it (the kit's layout review).
    pub fn paint_records_so_far(&self) -> &[(Rc<dyn Any>, Point<Pixels>)] {
        &self.next_frame.paint_records
    }

    /// Olo: paints what `paint` paints in `bounds`, or what it painted
    /// under `key` last frame, if nothing it depends on changed: `key`
    /// says all that (what it shows and how: hovered, selected, the
    /// time it says), and the clip, opacity, scale and size it was
    /// painted at must be the same. A window's `refresh` replays it all
    /// the same; `App::refresh_windows` (the theme, fonts) doesn't. In the same place it is replayed as a cached
    /// view is; moved by whole device pixels (scrolled), moved, if all it
    /// painted can be (`Scene::movable`: no paths, no listeners, nothing
    /// clipped inside it but by a clip of its own the outer one didn't cut
    /// the way it moved: a list row's cells). A canvas of many parts (Notifications' cards)
    /// draws again only the parts that changed: a hover redrew a whole
    /// screen of cards, 36,000 glyphs.
    pub fn paint_memo(
        &mut self,
        key: u64,
        bounds: Bounds<Pixels>,
        cx: &mut App,
        paint: impl FnOnce(&mut Window, &mut App),
    ) {
        self.invalidator.debug_assert_paint();
        let mask = self.content_mask();
        let opacity = self.element_opacity;
        let scale = self.scale_factor();
        if !self.memos_reset
            && paint_memos()
            && let Some(memo) = self.rendered_frame.memo(key)
            && memo.mask == mask
            && memo.opacity == opacity
            && memo.scale == scale
            && memo.bounds.size == bounds.size
        {
            let scale = self.scale_factor();
            let by = (bounds.origin - memo.bounds.origin).scale(scale);
            let whole = Point {
                x: ScaledPixels(by.x.0.round()),
                y: ScaledPixels(by.y.0.round()),
            };
            let on_grid = (by.x.0 - whole.x.0).abs() < 0.01 && (by.y.0 - whole.y.0).abs() < 0.01;
            let (range, movable) = (memo.range.clone(), memo.movable);
            let can =
                movable.is_some_and(|m| (whole.x.0 == 0.0 || m.x) && (whole.y.0 == 0.0 || m.y));
            // (Off the grid, by however little, it's painted afresh: a
            // fraction of a pixel left out of a move grows frame by frame.)
            if on_grid && (whole == Point::default() || can) {
                let start = self.paint_index();
                let mut movable = movable;
                if whole == Point::default() {
                    self.reuse_paint(range);
                } else if self.reuse_paint_moved(range, whole, bounds.origin - memo.bounds.origin) {
                    // (Cut where it landed: still to be replayed where it
                    // is, not moved again.)
                    movable = Some(crate::scene::Movable { x: false, y: false });
                }
                let end = self.paint_index();
                self.next_frame.keep_memo(
                    key,
                    PaintMemo {
                        bounds,
                        mask,
                        opacity,
                        scale,
                        range: start..end,
                        movable,
                    },
                );
                return;
            }
        }
        let start = self.paint_index();
        let culled = self.next_frame.scene.culled;
        let outer = std::mem::take(&mut self.paint_unfinished);
        paint(self, cx);
        let end = self.paint_index();
        // (Something clipped away whole isn't in what's kept: moved into
        // sight, it would be missing.)
        let whole = self.next_frame.scene.culled == culled;
        // (An unfinished part leaves what it's part of unfinished too.)
        let unfinished = self.paint_unfinished;
        self.paint_unfinished = outer || unfinished;
        if unfinished {
            return;
        }
        let movable = (whole
            && start.mouse_listeners_index == end.mouse_listeners_index
            && start.input_handlers_index == end.input_handlers_index
            && start.cursor_styles_index == end.cursor_styles_index
            && start.tab_handle_index == end.tab_handle_index)
            .then(|| {
                self.next_frame.scene.movable(
                    start.scene_index..end.scene_index,
                    &mask.scale(self.scale_factor()),
                )
            });
        self.next_frame.keep_memo(
            key,
            PaintMemo {
                bounds,
                mask,
                opacity,
                scale,
                range: start..end,
                movable,
            },
        );
    }

    /// Olo: something wasn't painted, clipped away whole (`Scene::culled`).
    pub(crate) fn note_culled(&mut self) {
        self.next_frame.scene.culled += 1;
    }

    /// Olo: cached view `view` is drawn again in the frame being drawn:
    /// what it shows was just given it by a view drawn before it (its
    /// parent's render handing it a tool bar's state), which a `notify`
    /// now would leave for the next frame. Outside a draw, it's `notify`.
    pub fn redraw_view(&mut self, view: EntityId, cx: &mut App) {
        if self.invalidator.not_drawing() {
            cx.notify(view);
        } else {
            self.dirty_views.insert(view);
        }
    }

    /// Olo: what's being painted isn't what will show (an icon or picture
    /// still loading, drawn when it comes): a [`Window::paint_memo`]
    /// painting it doesn't keep it.
    pub fn paint_unfinished(&mut self) {
        self.paint_unfinished = true;
    }

    /// [`Window::reuse_paint`] of a [`Scene::movable`] range, moved by
    /// `by` (`points` in logical pixels): its scene, what it noted for
    /// agents, and its lines kept.
    fn reuse_paint_moved(
        &mut self,
        range: Range<PaintIndex>,
        by: Point<ScaledPixels>,
        points: Point<Pixels>,
    ) -> bool {
        self.next_frame.paint_records.extend(
            self.rendered_frame.paint_records
                [range.start.paint_records_index..range.end.paint_records_index]
                .iter()
                .map(|(record, moved)| (record.clone(), *moved + points)),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);
        let mask = self.content_mask().scale(self.scale_factor());
        self.next_frame.scene.replay_moved(
            range.start.scene_index..range.end.scene_index,
            &self.rendered_frame.scene,
            by,
            &mask,
        )
    }

    pub(crate) fn reuse_paint(&mut self, range: Range<PaintIndex>) {
        // (The memos painted in it come along, where they now are: a
        // cached view replayed whole keeps its list's rows for the next
        // time it's drawn.)
        let to = self.paint_index();
        for (key, memo) in
            &self.rendered_frame.memos[range.start.memos_index..range.end.memos_index]
        {
            let memo = PaintMemo {
                range: memo.range.start.shifted(&range.start, &to)
                    ..memo.range.end.shifted(&range.start, &to),
                ..memo.clone()
            };
            self.next_frame.keep_memo(*key, memo);
        }
        self.next_frame.paint_records.extend(
            self.rendered_frame.paint_records
                [range.start.paint_records_index..range.end.paint_records_index]
                .iter()
                .cloned(),
        );
        self.next_frame.cursor_styles.extend(
            self.rendered_frame.cursor_styles
                [range.start.cursor_styles_index..range.end.cursor_styles_index]
                .iter()
                .cloned(),
        );
        self.next_frame.input_handlers.extend(
            self.rendered_frame.input_handlers
                [range.start.input_handlers_index..range.end.input_handlers_index]
                .iter_mut()
                .map(|handler| handler.take()),
        );
        self.next_frame.mouse_listeners.extend(
            self.rendered_frame.mouse_listeners
                [range.start.mouse_listeners_index..range.end.mouse_listeners_index]
                .iter_mut()
                .map(|listener| listener.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.next_frame.tab_stops.replay(
            &self.rendered_frame.tab_stops.insertion_history
                [range.start.tab_handle_index..range.end.tab_handle_index],
        );

        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);
        self.next_frame.scene.replay(
            range.start.scene_index..range.end.scene_index,
            &self.rendered_frame.scene,
        );
    }

    /// Push a text style onto the stack, and call a function with that style active.
    /// Use [`Window::text_style`] to get the current, combined text style. This method
    /// should only be called as part of element drawing.
    pub fn with_text_style<F, R>(&mut self, style: Option<TextStyleRefinement>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the cursor style at the platform level. This method should only be called
    /// during the prepaint phase of element drawing.
    pub fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: Some(hitbox.id),
            style,
        });
    }

    /// Updates the cursor style for the entire window at the platform level. A cursor
    /// style using this method will have precedence over any cursor style set using
    /// `set_cursor_style`. This method should only be called during the prepaint
    /// phase of element drawing.
    pub fn set_window_cursor_style(&mut self, style: CursorStyle) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: None,
            style,
        })
    }

    /// Sets a tooltip to be rendered for the upcoming frame. This method should only be called
    /// during the paint phase of element drawing.
    pub fn set_tooltip(&mut self, tooltip: AnyTooltip) -> TooltipId {
        self.invalidator.debug_assert_prepaint();
        let id = TooltipId(post_inc(&mut self.next_tooltip_id.0));
        self.next_frame
            .tooltip_requests
            .push(Some(TooltipRequest { id, tooltip }));
        id
    }

    /// Invoke the given function with the given content mask after intersecting it
    /// with the current mask. This method should only be called during element drawing.
    pub fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(mask) = mask {
            let mask = mask.intersect(&self.content_mask());
            self.content_mask_stack.push(mask);
            let result = f(self);
            self.content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the global element offset relative to the current offset. This is used to implement
    /// scrolling. This method should only be called during the prepaint phase of element drawing.
    pub fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();

        if offset.is_zero() {
            return f(self);
        };

        let abs_offset = self.element_offset() + offset;
        self.with_absolute_element_offset(abs_offset, f)
    }

    /// Updates the global element offset based on the given offset. This is used to implement
    /// drag handles and other manual painting of elements. This method should only be called during
    /// the prepaint phase of element drawing.
    pub fn with_absolute_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack.push(offset);
        let result = f(self);
        self.element_offset_stack.pop();
        result
    }

    /// Olo: paints what `f` paints at `opacity` (multiplied into any
    /// opacity already applied): a disabled control, faded.
    pub fn with_element_opacity<R>(
        &mut self,
        opacity: Option<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();

        let Some(opacity) = opacity else {
            return f(self);
        };

        let previous_opacity = self.element_opacity;
        self.element_opacity = previous_opacity * opacity;
        let result = f(self);
        self.element_opacity = previous_opacity;
        result
    }

    /// Perform prepaint on child elements in a "retryable" manner, so that any side effects
    /// of prepaints can be discarded before prepainting again. This is used to support autoscroll
    /// where we need to prepaint children to detect the autoscroll bounds, then adjust the
    /// element offset and prepaint again. See [`crate::List`] for an example. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn transact<T, U>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, U>) -> Result<T, U> {
        self.invalidator.debug_assert_prepaint();
        let index = self.prepaint_index();
        let result = f(self);
        if result.is_err() {
            self.next_frame.hitboxes.truncate(index.hitboxes_index);
            self.next_frame
                .tooltip_requests
                .truncate(index.tooltips_index);
            self.next_frame
                .deferred_draws
                .truncate(index.deferred_draws_index);
            self.next_frame
                .dispatch_tree
                .truncate(index.dispatch_tree_index);
            self.next_frame
                .accessed_element_states
                .truncate(index.accessed_element_states_index);
            self.text_system.truncate_layouts(index.line_layout_index);
        }
        result
    }

    /// When you call this method during [`Element::prepaint`], containing elements will attempt to
    /// scroll to cause the specified bounds to become visible. When they decide to autoscroll, they will call
    /// [`Element::prepaint`] again with a new set of bounds. See [`crate::List`] for an example of an element
    /// that supports this method being called on the elements it contains. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn request_autoscroll(&mut self, bounds: Bounds<Pixels>) {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll = Some(bounds);
    }

    /// This method can be called from a containing element such as [`crate::List`] to support the autoscroll behavior
    /// described in [`Self::request_autoscroll`].
    pub fn take_autoscroll(&mut self) -> Option<Bounds<Pixels>> {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll.take()
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    /// Your view will be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn use_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, is_first) = cx.fetch_asset::<A>(source);
        task.clone().now_or_never().or_else(|| {
            if is_first {
                let entity_id = self.current_view();
                self.spawn(cx, {
                    let task = task.clone();
                    async move |cx| {
                        task.await;

                        cx.on_next_frame(move |_, cx| {
                            cx.notify(entity_id);
                        });
                    }
                })
                .detach();
            }

            None
        })
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading or doesn't exist this will return None.
    /// Your view will not be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn get_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, _) = cx.fetch_asset::<A>(source);
        task.now_or_never()
    }
    /// Obtain the current element offset. This method should only be called during the
    /// prepaint phase of element drawing.
    pub fn element_offset(&self) -> Point<Pixels> {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack
            .last()
            .copied()
            .unwrap_or_default()
    }

    /// Obtain the current element opacity. This method should only be called during the
    /// prepaint phase of element drawing.
    #[inline]
    pub(crate) fn element_opacity(&self) -> f32 {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.element_opacity
    }

    /// Obtain the current content mask. This method should only be called during element drawing.
    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.content_mask_stack
            .last()
            .cloned()
            .unwrap_or_else(|| ContentMask {
                bounds: Bounds {
                    origin: Point::default(),
                    size: self.viewport_size,
                },
            })
    }

    /// Provide elements in the called function with a new namespace in which their identifiers must be unique.
    /// This can be used within a custom element to distinguish multiple sets of child elements.
    pub fn with_element_namespace<R>(
        &mut self,
        element_id: impl Into<ElementId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames.
    pub fn use_keyed_state<S: 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        let current_view = self.current_view();
        self.with_global_id(key.into(), |global_id, window| {
            window.with_element_state(global_id, |state: Option<Entity<S>>, window| {
                if let Some(state) = state {
                    (state.clone(), state)
                } else {
                    let new_state = cx.new(|cx| init(window, cx));
                    cx.observe(&new_state, move |_, cx| {
                        cx.notify(current_view);
                    })
                    .detach();
                    (new_state.clone(), new_state)
                }
            })
        })
    }

    /// Immediately push an element ID onto the stack. Useful for simplifying IDs in lists
    pub fn with_id<R>(&mut self, id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.with_global_id(id.into(), |_, window| f(window))
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames, without needing to specify a key
    ///
    /// NOTE: This method uses the location of the caller to generate an ID for this state.
    ///       If this is not sufficient to identify your state (e.g. you're rendering a list item),
    ///       you can provide a custom ElementID using the `use_keyed_state` method.
    #[track_caller]
    pub fn use_state<S: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        self.use_keyed_state(
            ElementId::CodeLocation(*core::panic::Location::caller()),
            cx,
            init,
        )
    }

    /// Updates or initializes state for an element with the given id that lives across multiple
    /// frames. If an element with this ID existed in the rendered frame, its state will be passed
    /// to the given closure. The state returned by the closure will be stored so it can be referenced
    /// when drawing the next frame. This method should only be called as part of element drawing.
    pub fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        let key = (GlobalElementId(global_id.0.clone()), TypeId::of::<S>());
        self.next_frame
            .accessed_element_states
            .push((GlobalElementId(key.0.clone()), TypeId::of::<S>()));

        if let Some(any) = self
            .next_frame
            .element_states
            .remove(&key)
            .or_else(|| self.rendered_frame.element_states.remove(&key))
        {
            let ElementStateBox {
                inner,
                #[cfg(debug_assertions)]
                type_name,
            } = any;
            // Using the extra inner option to avoid needing to reallocate a new box.
            let mut state_box = inner
                .downcast::<Option<S>>()
                .map_err(|_| {
                    #[cfg(debug_assertions)]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}, actual: {:?}",
                            std::any::type_name::<S>(),
                            type_name
                        )
                    }

                    #[cfg(not(debug_assertions))]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}",
                            std::any::type_name::<S>(),
                        )
                    }
                })
                .unwrap();

            let state = state_box.take().expect(
                "reentrant call to with_element_state for the same state type and element id",
            );
            let (result, state) = f(Some(state), self);
            state_box.replace(state);
            self.next_frame.element_states.insert(
                key,
                ElementStateBox {
                    inner: state_box,
                    #[cfg(debug_assertions)]
                    type_name,
                },
            );
            result
        } else {
            let (result, state) = f(None, self);
            self.next_frame.element_states.insert(
                key,
                ElementStateBox {
                    inner: Box::new(Some(state)),
                    #[cfg(debug_assertions)]
                    type_name: std::any::type_name::<S>(),
                },
            );
            result
        }
    }

    /// A variant of `with_element_state` that allows the element's id to be optional. This is a convenience
    /// method for elements where the element id may or may not be assigned. Prefer using `with_element_state`
    /// when the element is guaranteed to have an id.
    ///
    /// The first option means 'no ID provided'
    /// The second option means 'not yet initialized'
    pub fn with_optional_element_state<S, R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(global_id) = global_id {
            self.with_element_state(global_id, |state, cx| {
                let (result, state) = f(Some(state), cx);
                let state =
                    state.expect("you must return some state when you pass some element id");
                (result, state)
            })
        } else {
            let (result, state) = f(None, self);
            debug_assert!(
                state.is_none(),
                "you must not return an element state when passing None for the global id"
            );
            result
        }
    }

    /// Executes the given closure within the context of a tab group.
    #[inline]
    pub fn with_tab_group<R>(&mut self, index: Option<isize>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(index) = index {
            self.next_frame.tab_stops.begin_group(index);
            let result = f(self);
            self.next_frame.tab_stops.end_group();
            result
        } else {
            f(self)
        }
    }

    /// Defers the drawing of the given element, scheduling it to be painted on top of the currently-drawn tree
    /// at a later time. The `priority` parameter determines the drawing order relative to other deferred elements,
    /// with higher values being drawn on top.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn defer_draw(
        &mut self,
        element: AnyElement,
        absolute_offset: Point<Pixels>,
        priority: usize,
    ) {
        self.invalidator.debug_assert_prepaint();
        let parent_node = self.next_frame.dispatch_tree.active_node_id().unwrap();
        self.next_frame.deferred_draws.push(DeferredDraw {
            current_view: self.current_view(),
            parent_node,
            element_id_stack: self.element_id_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            priority,
            element: Some(element),
            absolute_offset,
            prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
            paint_range: PaintIndex::default()..PaintIndex::default(),
        });
    }

    /// Creates a new painting layer for the specified bounds. A "layer" is a batch
    /// of geometry that are non-overlapping and have the same draw order. This is typically used
    /// for performance reasons.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let clipped_bounds = bounds.intersect(&content_mask.bounds);
        if !clipped_bounds.is_empty() {
            self.next_frame
                .scene
                .push_layer(clipped_bounds.scale(scale_factor));
        }

        let result = f(self);

        if !clipped_bounds.is_empty() {
            self.next_frame.scene.pop_layer();
        }

        result
    }

    /// Paint one or more drop shadows into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        for shadow in shadows {
            let shadow_bounds = (bounds + shadow.offset).dilate(shadow.spread_radius);
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: shadow_bounds.scale(scale_factor),
                content_mask: content_mask.scale(scale_factor),
                corner_radii: corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
            });
        }
    }

    /// Paint one or more quads into the scene for the next frame at the current stacking context.
    /// Quads are colored rectangular regions with an optional background, border, and corner radius.
    /// see [`fill`], [`outline`], and [`quad`] to construct this type.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// Note that the `quad.corner_radii` are allowed to exceed the bounds, creating sharp corners
    /// where the circular arcs meet. This will not display well when combined with dashed borders.
    /// Use `Corners::clamp_radii_for_quad_size` if the radii should fit within the bounds.
    pub fn paint_quad(&mut self, quad: PaintQuad) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        self.next_frame.scene.insert_primitive(Quad {
            order: 0,
            bounds: quad.bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(scale_factor),
            border_widths: quad.border_widths.scale(scale_factor),
            border_style: quad.border_style,
        });
    }

    /// Paint the given `Path` into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_path(&mut self, mut path: Path<Pixels>, color: impl Into<Background>) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        let color: Background = color.into();
        // Olo: a small one in a plain colour, with exact coverage.
        if let Some(outline) = path.outline.take()
            && color.tag == crate::BackgroundTag::Solid
            && let Some(placed) = crate::outline::place(&outline, path.bounds, scale_factor)
            && let Ok(tile) = self.outlines.tile(self.sprite_atlas.as_ref(), &placed.key)
        {
            if let Some(tile) = tile {
                self.next_frame.scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    coverage: 1,
                    bounds: placed.bounds,
                    content_mask: content_mask.scale(scale_factor),
                    color: color.solid.opacity(opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
            return;
        }
        path.content_mask = content_mask;
        path.color = color.opacity(opacity);
        self.next_frame
            .scene
            .insert_primitive(path.scale(scale_factor));
    }

    /// Olo: paints a straight line from `from` to `to`, `width` wide, its
    /// ends square (`round`: rounded) and its edges antialiased by each
    /// pixel's distance to it, drawn on the GPU (graphs, screen savers,
    /// strokes): where a path would be tessellated, one instance.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_segment(
        &mut self,
        from: Point<Pixels>,
        to: Point<Pixels>,
        width: Pixels,
        round: bool,
        color: Hsla,
    ) {
        self.invalidator.debug_assert_paint();
        let scale = self.scale_factor();
        let (a, b) = (from.scale(scale), to.scale(scale));
        let half = width.scale(scale) / 2.0;
        let reach = half + ScaledPixels(1.0) + if round { half } else { ScaledPixels(0.0) };
        let min = point(a.x.min(b.x) - reach, a.y.min(b.y) - reach);
        let max = point(a.x.max(b.x) + reach, a.y.max(b.y) + reach);
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.next_frame.scene.insert_primitive(Segment {
            order: 0,
            round: round as u32,
            bounds: Bounds::from_corners(min, max),
            content_mask,
            color: color.opacity(opacity),
            from: a,
            to: b,
            half_width: half,
            bottom: ScaledPixels(0.0),
        });
    }

    /// Olo: paints straight lines, each piece `width` wide, their ends
    /// square (`round`: rounded, so the pieces of a polyline join), in
    /// one colour: [`Window::paint_segment`] for many, the clip, opacity
    /// and scale looked up once (a curve of a thousand pieces).
    pub fn paint_segments(
        &mut self,
        pieces: &[(Point<Pixels>, Point<Pixels>)],
        width: Pixels,
        round: bool,
        color: Hsla,
    ) {
        self.invalidator.debug_assert_paint();
        let scale = self.scale_factor();
        let half = width.scale(scale) / 2.0;
        let reach = half + ScaledPixels(1.0) + if round { half } else { ScaledPixels(0.0) };
        let color = color.opacity(self.element_opacity());
        let content_mask = self.content_mask().scale(scale);
        let segments = pieces
            .iter()
            .map(|(from, to)| {
                let (a, b) = (from.scale(scale), to.scale(scale));
                Segment {
                    order: 0,
                    round: round as u32,
                    bounds: Bounds::from_corners(
                        point(a.x.min(b.x) - reach, a.y.min(b.y) - reach),
                        point(a.x.max(b.x) + reach, a.y.max(b.y) + reach),
                    ),
                    content_mask: content_mask.clone(),
                    color,
                    from: a,
                    to: b,
                    half_width: half,
                    bottom: ScaledPixels(0.0),
                }
            })
            .collect();
        self.next_frame.scene.insert_segments(segments);
    }

    /// Olo: paints what's under the line from `from` to `to` (left to
    /// right), down to `bottom`: a graph's fill, a column of it a
    /// segment. Drawn on the GPU in one instance, its slanted top
    /// antialiased by each pixel's distance to the line, and its sides
    /// taking the pixels whose middles they hold, so areas side by side
    /// share each pixel once (no seam, nothing drawn twice) — where a path
    /// would be tessellated, or filled multisampled through textures as
    /// large as the window.
    pub fn paint_area(
        &mut self,
        from: Point<Pixels>,
        to: Point<Pixels>,
        bottom: Pixels,
        color: Hsla,
    ) {
        self.invalidator.debug_assert_paint();
        let scale = self.scale_factor();
        let (a, b, bottom) = (from.scale(scale), to.scale(scale), bottom.scale(scale));
        if b.x <= a.x {
            return;
        }
        let top = a.y.min(b.y) - ScaledPixels(1.0);
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.next_frame.scene.insert_primitive(Segment {
            order: 0,
            round: 2,
            bounds: Bounds::from_corners(point(a.x, top), point(b.x, bottom.max(top))),
            content_mask,
            color: color.opacity(opacity),
            from: a,
            to: b,
            half_width: ScaledPixels(0.0),
            bottom,
        });
    }

    /// Olo: paints a slice of a ring around `center`, from radius `inner`
    /// to `outer` and from `start` radians clockwise from 12 o'clock
    /// `sweep` radians round (a whole turn or more: the whole ring; inner
    /// 0: a pie's slice, or a disc), its edges antialiased by each pixel's
    /// distance to them, drawn on the GPU: one instance, where a path
    /// would be tessellated or rasterised (ring charts, gauges, dials,
    /// round marks, a circle's outline as a ring a line wide).
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_sector(
        &mut self,
        center: Point<Pixels>,
        inner: Pixels,
        outer: Pixels,
        start: f32,
        sweep: f32,
        color: Hsla,
    ) {
        self.invalidator.debug_assert_paint();
        if !(sweep > 0.0) || !(outer > inner) {
            return;
        }
        let scale = self.scale_factor();
        let c = center.scale(scale);
        let (r0, r1) = (
            inner.scale(scale).max(ScaledPixels(0.0)),
            outer.scale(scale),
        );
        // What it covers: its corners (the middle, with no hole), and the
        // ring's extremes at the quarter turns it passes, a pixel more for
        // the edges.
        let tau = std::f32::consts::TAU;
        let at = |r: ScaledPixels, a: f32| point(c.x + r * a.sin(), c.y - r * a.cos());
        let (mut min, mut max) = (
            point(ScaledPixels(f32::MAX), ScaledPixels(f32::MAX)),
            point(ScaledPixels(f32::MIN), ScaledPixels(f32::MIN)),
        );
        let mut take = |p: Point<ScaledPixels>| {
            min = point(min.x.min(p.x), min.y.min(p.y));
            max = point(max.x.max(p.x), max.y.max(p.y));
        };
        if sweep >= tau {
            take(point(c.x - r1, c.y - r1));
            take(point(c.x + r1, c.y + r1));
        } else {
            let end = start + sweep;
            for a in [start, end] {
                take(at(r0, a));
                take(at(r1, a));
            }
            let quarter = tau / 4.0;
            let mut k = (start / quarter).ceil();
            while k * quarter <= end {
                take(at(r1, k * quarter));
                k += 1.0;
            }
        }
        let pad = ScaledPixels(1.0);
        let bounds = Bounds::from_corners(
            point(min.x - pad, min.y - pad),
            point(max.x + pad, max.y + pad),
        );
        let opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale);
        self.next_frame.scene.insert_primitive(Sector {
            order: 0,
            pad: 0,
            bounds,
            content_mask,
            color: color.opacity(opacity),
            center: c,
            inner: r0,
            outer: r1,
            start,
            sweep: sweep.min(tau),
        });
    }

    /// Paint an underline into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_underline(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let height = if style.wavy {
            style.thickness * 3.
        } else {
            style.thickness
        };
        let bounds = Bounds {
            origin,
            size: size(width, height),
        };
        let content_mask = self.content_mask();
        let element_opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(element_opacity),
            thickness: style.thickness.scale(scale_factor),
            wavy: if style.wavy { 1 } else { 0 },
        });
    }

    /// Paint a strikethrough into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_strikethrough(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &StrikethroughStyle,
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let height = style.thickness;
        let bounds = Bounds {
            origin,
            size: size(width, height),
        };
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            thickness: style.thickness.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(opacity),
            wavy: 0,
        });
    }

    /// Paints a monochrome (non-emoji) glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single glyph that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);

        let subpixel_variant = Point {
            x: (glyph_origin.x.0.fract() * SUBPIXEL_VARIANTS_X as f32).floor() as u8,
            y: (glyph_origin.y.0.fract() * SUBPIXEL_VARIANTS_Y as f32).floor() as u8,
        };
        let key = (
            font_id.0 as u32,
            glyph_id.0,
            font_size.0.to_bits(),
            subpixel_variant.x,
            subpixel_variant.y,
        );
        let found = match self.glyph_memo.get(&key) {
            Some(found) => found.clone(),
            None => {
                let params = RenderGlyphParams {
                    font_id,
                    glyph_id,
                    font_size,
                    subpixel_variant,
                    scale_factor,
                    is_emoji: false,
                    rendering: self.platform_window.text_rendering(),
                };
                let raster_bounds = self.text_system().raster_bounds(&params)?;
                let found = if raster_bounds.is_zero() {
                    None
                } else {
                    let tile = self
                        .sprite_atlas
                        .get_or_insert_with(&params.clone().into(), &mut || {
                            // Olo: counted, so a face drawn as running text has
                            // the rest of its glyphs drawn ahead, off this thread.
                            self.text_system().drawn_first(
                                params.font_id,
                                params.font_size,
                                params.scale_factor,
                                params.rendering,
                            );
                            let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                            Ok(Some((size, Cow::Owned(bytes))))
                        })?
                        .expect("Callback above only errors or returns Some");
                    Some((raster_bounds, tile))
                };
                self.glyph_memo.insert(key, found.clone());
                found
            }
        };
        if let Some((raster_bounds, tile)) = found {
            let bounds = Bounds {
                origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.content_mask().scale(scale_factor);
            self.next_frame.scene.insert_primitive(MonochromeSprite {
                order: 0,
                coverage: 0,
                bounds,
                content_mask,
                color: color.opacity(element_opacity),
                tile,
                transformation: TransformationMatrix::unit(),
            });
        }
        Ok(())
    }

    /// Olo: paints many glyphs at once, each at its baseline origin in
    /// its face, size and colour: as [`Window::paint_glyph`] each, but
    /// one paint operation for them all (a page of text).
    pub fn paint_glyphs(
        &mut self,
        glyphs: impl IntoIterator<Item = (Point<Pixels>, FontId, GlyphId, Pixels, Hsla)>,
    ) {
        self.invalidator.debug_assert_paint();
        let element_opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(self.scale_factor());
        let mut sprites = Vec::new();
        for (origin, font_id, glyph_id, font_size, color) in glyphs {
            if let Some((bounds, tile)) = self.glyph_sprite(origin, font_id, glyph_id, font_size) {
                sprites.push(MonochromeSprite {
                    order: 0,
                    coverage: 0,
                    bounds,
                    content_mask: content_mask.clone(),
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
        }
        self.next_frame.scene.insert_sprites(sprites);
    }

    /// Olo: what's being dragged over the window, if anything (from
    /// another program or one of this one's windows).
    pub fn drop_offer(&self) -> Option<&crate::DropOffer> {
        self.drop_offer.as_ref()
    }

    /// Olo: take what's dragged over the window here: `target` names the
    /// part (it comes back in [`crate::Dropped::target`]), `mime` is one
    /// of the offer's types, `action` one its source allows. Called
    /// each frame the drag is over the part (from its paint); a frame
    /// without it refuses the drag. The last call wins. Only a window
    /// with a drop handler ([`Window::on_dropped`]) takes drops this way.
    pub fn accept_drop(
        &mut self,
        target: impl Into<String>,
        mime: impl Into<String>,
        action: crate::DragAction,
    ) {
        self.drop_accept = Some(crate::DropAccept {
            target: target.into(),
            mime: mime.into(),
            action,
        });
    }

    /// Olo: what's dropped where the window took it comes to `handler`,
    /// its data read.
    pub fn on_dropped(
        &mut self,
        handler: impl FnMut(crate::Dropped, &mut Window, &mut App) + 'static,
    ) {
        self.drop_handler = Some(Rc::new(RefCell::new(handler)));
    }

    /// Olo: paints `run`'s glyphs (as [`Window::paint_glyphs`]), each at
    /// its offset from `origin`. The first time (and when the window's
    /// scale or text rendering, the caller's `stamp` for what `glyphs`
    /// gives, or where `origin` falls within a device pixel changes) they
    /// are looked up in the atlas; after that the run is painted from
    /// what was found, moved, without a lookup a glyph: a page of text
    /// scrolled by whole pixels is tens of thousands of lookups a frame
    /// otherwise.
    pub fn paint_glyph_run(
        &mut self,
        run: &mut GlyphRun,
        origin: Point<Pixels>,
        stamp: u64,
        glyphs: impl FnOnce(&mut dyn FnMut(Point<Pixels>, FontId, GlyphId, Pixels, Hsla)),
    ) {
        self.invalidator.debug_assert_paint();
        let scale_factor = self.scale_factor();
        let device = origin.scale(scale_factor);
        // Its whole device pixel and where within it (in 64ths).
        let mut base = Point {
            x: device.x.0.floor(),
            y: device.y.0.floor(),
        };
        let mut phase = (
            ((device.x.0 - base.x) * 64.0).round() as i32,
            ((device.y.0 - base.y) * 64.0).round() as i32,
        );
        if phase.0 == 64 {
            base.x += 1.0;
            phase.0 = 0;
        }
        if phase.1 == 64 {
            base.y += 1.0;
            phase.1 = 0;
        }
        let key = GlyphRunKey {
            scale: scale_factor.to_bits(),
            rendering: self.platform_window.text_rendering(),
            stamp,
            phase,
        };
        if run.key != Some(key) {
            let cold = frame_phases().then(std::time::Instant::now);
            run.key = Some(key);
            run.sprites.clear();
            let mut found = Vec::new();
            glyphs(&mut |at, font_id, glyph_id, font_size, color| {
                found.push((origin + at, font_id, glyph_id, font_size, color))
            });
            for (at, font_id, glyph_id, font_size, color) in found {
                if let Some((bounds, tile)) = self.glyph_sprite(at, font_id, glyph_id, font_size) {
                    let rel = Bounds {
                        origin: Point {
                            x: ScaledPixels(bounds.origin.x.0 - base.x),
                            y: ScaledPixels(bounds.origin.y.0 - base.y),
                        },
                        size: bounds.size,
                    };
                    run.sprites.push((rel, tile, color));
                }
            }
            if let Some(t) = cold {
                FRAME_WORK.note(&FRAME_WORK.runs, t);
            }
        }
        let element_opacity = self.element_opacity();
        let content_mask = self.content_mask().scale(scale_factor);
        let sprites = run
            .sprites
            .iter()
            .map(|(rel, tile, color)| MonochromeSprite {
                order: 0,
                coverage: 0,
                bounds: Bounds {
                    origin: Point {
                        x: ScaledPixels(rel.origin.x.0 + base.x),
                        y: ScaledPixels(rel.origin.y.0 + base.y),
                    },
                    size: rel.size,
                },
                content_mask: content_mask.clone(),
                color: color.opacity(element_opacity),
                tile: tile.clone(),
                transformation: TransformationMatrix::unit(),
            });
        self.next_frame.scene.insert_sprites(sprites);
    }

    /// A glyph at `origin` (its baseline): where its sprite goes, in
    /// device pixels, and its tile (looked up once a frame a glyph).
    fn glyph_sprite(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Option<(Bounds<ScaledPixels>, AtlasTile)> {
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let subpixel_variant = Point {
            x: (glyph_origin.x.0.fract() * SUBPIXEL_VARIANTS_X as f32).floor() as u8,
            y: (glyph_origin.y.0.fract() * SUBPIXEL_VARIANTS_Y as f32).floor() as u8,
        };
        let key = (
            font_id.0 as u32,
            glyph_id.0,
            font_size.0.to_bits(),
            subpixel_variant.x,
            subpixel_variant.y,
        );
        let found = match self.glyph_memo.get(&key) {
            Some(found) => found.clone(),
            None => {
                let params = RenderGlyphParams {
                    font_id,
                    glyph_id,
                    font_size,
                    subpixel_variant,
                    scale_factor,
                    is_emoji: false,
                    rendering: self.platform_window.text_rendering(),
                };
                let found = match self.text_system().raster_bounds(&params) {
                    Ok(b) if !b.is_zero() => self
                        .sprite_atlas
                        .get_or_insert_with(&params.clone().into(), &mut || {
                            self.text_system().drawn_first(
                                params.font_id,
                                params.font_size,
                                params.scale_factor,
                                params.rendering,
                            );
                            let t = frame_phases().then(std::time::Instant::now);
                            let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                            if let Some(t) = t {
                                FRAME_WORK.note(&FRAME_WORK.rasterized, t);
                            }
                            Ok(Some((size, Cow::Owned(bytes))))
                        })
                        .ok()
                        .flatten()
                        .map(|tile| (b, tile)),
                    _ => None,
                };
                self.glyph_memo.insert(key, found.clone());
                found
            }
        };
        let (raster_bounds, tile) = found?;
        Some((
            Bounds {
                origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            },
            tile,
        ))
    }

    /// Paints an emoji glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single emoji that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_emoji(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            // We don't render emojis with subpixel variants.
            subpixel_variant: Default::default(),
            scale_factor,
            is_emoji: true,
            rendering: TextRendering::default(),
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let tile = self
                .sprite_atlas
                .get_or_insert_with(&params.clone().into(), &mut || {
                    let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
                .expect("Callback above only errors or returns Some");

            let bounds = Bounds {
                origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.content_mask().scale(scale_factor);
            let opacity = self.element_opacity();

            self.next_frame.scene.insert_primitive(PolychromeSprite {
                order: 0,
                nearest: 0,
                grayscale: false,
                bounds,
                corner_radii: Default::default(),
                content_mask,
                tile,
                opacity,
            });
        }
        Ok(())
    }

    /// Paint a monochrome SVG into the scene for the next frame at the current stacking context.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_svg(
        &mut self,
        bounds: Bounds<Pixels>,
        path: SharedString,
        transformation: TransformationMatrix,
        color: Hsla,
        cx: &App,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();

        let bounds = bounds.scale(scale_factor);
        let params = RenderSvgParams {
            path,
            size: bounds.size.map(|pixels| {
                DevicePixels::from((pixels.0 * SMOOTH_SVG_SCALE_FACTOR).ceil() as i32)
            }),
        };

        let Some(tile) =
            self.sprite_atlas
                .get_or_insert_with(&params.clone().into(), &mut || {
                    let Some((size, bytes)) = cx.svg_renderer.render(&params)? else {
                        return Ok(None);
                    };
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
        else {
            return Ok(());
        };
        let content_mask = self.content_mask().scale(scale_factor);
        let svg_bounds = Bounds {
            origin: bounds.center()
                - Point::new(
                    ScaledPixels(tile.bounds.size.width.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.),
                    ScaledPixels(tile.bounds.size.height.0 as f32 / SMOOTH_SVG_SCALE_FACTOR / 2.),
                ),
            size: tile
                .bounds
                .size
                .map(|value| ScaledPixels(value.0 as f32 / SMOOTH_SVG_SCALE_FACTOR)),
        };

        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            coverage: 0,
            bounds: svg_bounds
                .map_origin(|origin| origin.round())
                .map_size(|size| size.ceil()),
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        });

        Ok(())
    }

    /// Paint an image into the scene for the next frame at the current z-index.
    /// This method will panic if the frame_index is not valid
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_image(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
    ) -> Result<()> {
        self.paint_image_sampled(bounds, corner_radii, data, frame_index, grayscale, false)
    }

    /// Olo: [`Window::paint_image`], its texels shown as they are when
    /// magnified (nearest), not blended with their neighbours: a small
    /// picture (an icon) looked at closely keeps its pixels.
    pub fn paint_image_nearest(
        &mut self,
        bounds: Bounds<Pixels>,
        data: Arc<RenderImage>,
    ) -> Result<()> {
        self.paint_image_sampled(bounds, Corners::default(), data, 0, false, true)
    }

    /// Olo: paints triangles of an image, each corner a point in the
    /// window and where in the image (0–1 across it) it shows, the image
    /// tinted (white leaves it as it is): a picture bent in perspective
    /// (the Flip screen saver), a lens's magnified disc. On the GPU, one
    /// instance a triangle, sampled from the image's atlas tile.
    pub fn paint_image_triangles(
        &mut self,
        data: Arc<RenderImage>,
        triangles: &[[(Point<Pixels>, Point<f32>); 3]],
        tint: Hsla,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();
        let scale_factor = self.scale_factor();
        let params = RenderImageParams {
            image_id: data.id,
            frame_index: 0,
            deep: data.is_deep(),
        };
        let tile = self
            .sprite_atlas
            .get_or_insert_with(&params.into(), &mut || {
                Ok(Some((
                    data.size(0),
                    Cow::Borrowed(data.as_bytes(0).expect("an image has a first frame")),
                )))
            })?
            .expect("Callback above only returns Some");
        let content_mask = self.content_mask().scale(scale_factor);
        let opacity = self.element_opacity();
        for t in triangles {
            let corners = [
                t[0].0.scale(scale_factor),
                t[1].0.scale(scale_factor),
                t[2].0.scale(scale_factor),
            ];
            let min = point(
                corners[0].x.min(corners[1].x).min(corners[2].x),
                corners[0].y.min(corners[1].y).min(corners[2].y),
            );
            let max = point(
                corners[0].x.max(corners[1].x).max(corners[2].x),
                corners[0].y.max(corners[1].y).max(corners[2].y),
            );
            self.next_frame
                .scene
                .insert_primitive(crate::ImageTriangle {
                    order: 0,
                    opacity,
                    bounds: Bounds::from_corners(min, max),
                    content_mask: content_mask.clone(),
                    tile: tile.clone(),
                    corners,
                    uvs: [t[0].1, t[1].1, t[2].1],
                    tint,
                });
        }
        Ok(())
    }

    fn paint_image_sampled(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
        nearest: bool,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let params = RenderImageParams {
            image_id: data.id,
            frame_index,
            deep: data.is_deep(),
        };

        let tile = self
            .sprite_atlas
            .get_or_insert_with(&params.into(), &mut || {
                Ok(Some((
                    data.size(frame_index),
                    Cow::Borrowed(
                        data.as_bytes(frame_index)
                            .expect("It's the caller's job to pass a valid frame index"),
                    ),
                )))
            })?
            .expect("Callback above only returns Some");
        let content_mask = self.content_mask().scale(scale_factor);
        let corner_radii = corner_radii.scale(scale_factor);
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(PolychromeSprite {
            order: 0,
            // (Bit 1: the image is Display P3's numbers; bit 2, HDR10's.)
            nearest: u32::from(nearest)
                | match data.color_space {
                    crate::Gamut::DisplayP3 => 2,
                    crate::Gamut::Hdr10 => 4,
                    crate::Gamut::Srgb => 0,
                },
            grayscale,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask,
            corner_radii,
            tile,
            opacity,
        });
        Ok(())
    }

    /// Olo: paint a video's picture as decoded (Y'CbCr planes: `frame`)
    /// into `bounds`, turned into RGB by the GPU. A stream's frames keep
    /// its textures (`YuvFrame::stream`), filled again for each new one.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    #[cfg(not(target_os = "macos"))]
    pub fn paint_yuv(&mut self, bounds: Bounds<Pixels>, frame: std::sync::Arc<crate::YuvFrame>) {
        use crate::PaintSurface;

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask,
            frame,
        });
    }

    /// Paint a surface into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    #[cfg(target_os = "macos")]
    pub fn paint_surface(&mut self, bounds: Bounds<Pixels>, image_buffer: CVPixelBuffer) {
        use crate::PaintSurface;

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask,
            image_buffer,
        });
    }

    /// Removes an image from the sprite atlas.
    pub fn drop_image(&mut self, data: Arc<RenderImage>) -> Result<()> {
        for frame_index in 0..data.frame_count() {
            let params = RenderImageParams {
                image_id: data.id,
                frame_index,
                deep: data.is_deep(),
            };

            self.sprite_atlas.remove(&params.clone().into());
        }

        Ok(())
    }

    /// Add a node to the layout tree for the current frame. Takes the `Style` of the element for which
    /// layout is being requested, along with the layout ids of any children. This method is called during
    /// calls to the [`Element::request_layout`] trait method and enables any element to participate in layout.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    #[must_use]
    pub fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        cx: &mut App,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        cx.layout_id_buffer.clear();
        cx.layout_id_buffer.extend(children);
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();

        self.layout_engine.as_mut().unwrap().request_layout(
            style,
            rem_size,
            scale_factor,
            &cx.layout_id_buffer,
        )
    }

    /// Add a node to the layout tree for the current frame. Instead of taking a `Style` and children,
    /// this variant takes a function that is invoked during layout so you can use arbitrary logic to
    /// determine the element's size. One place this is used internally is when measuring text.
    ///
    /// The given closure is invoked at layout time with the known dimensions and available space and
    /// returns a `Size`.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    pub fn request_measured_layout<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        style: Style,
        measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_measured_layout(style, rem_size, scale_factor, measure)
    }

    /// Compute the layout for the given id within the given available space.
    /// This method is called for its side effect, typically by the framework prior to painting.
    /// After calling it, you can request the bounds of the given layout node id or any descendant.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn compute_layout(
        &mut self,
        layout_id: LayoutId,
        available_space: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        self.invalidator.debug_assert_prepaint();

        let mut layout_engine = self.layout_engine.take().unwrap();
        layout_engine.compute_layout(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    /// Obtain the bounds computed for the given LayoutId relative to the window. This method will usually be invoked by
    /// GPUI itself automatically in order to pass your element its `Bounds` automatically.
    ///
    /// This method should only be called as part of element drawing.
    pub fn layout_bounds(&mut self, layout_id: LayoutId) -> Bounds<Pixels> {
        self.invalidator.debug_assert_prepaint();

        let scale_factor = self.scale_factor();
        let mut bounds = self
            .layout_engine
            .as_mut()
            .unwrap()
            .layout_bounds(layout_id, scale_factor)
            .map(Into::into);
        bounds.origin += self.element_offset();
        bounds
    }

    /// This method should be called during `prepaint`. You can use
    /// the returned [Hitbox] during `paint` or in an event handler
    /// to determine whether the inserted hitbox was the topmost.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn insert_hitbox(&mut self, bounds: Bounds<Pixels>, behavior: HitboxBehavior) -> Hitbox {
        self.invalidator.debug_assert_prepaint();

        let content_mask = self.content_mask();
        let mut id = self.next_hitbox_id;
        self.next_hitbox_id = self.next_hitbox_id.next();
        let hitbox = Hitbox {
            id,
            bounds,
            content_mask,
            behavior,
        };
        self.next_frame.hitboxes.push(hitbox.clone());
        hitbox
    }

    /// Set a hitbox which will act as a control area of the platform window.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn insert_window_control_hitbox(&mut self, area: WindowControlArea, hitbox: Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.window_control_hitboxes.push((area, hitbox));
    }

    /// Sets the key context for the current element. This context will be used to translate
    /// keybindings into actions.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn set_key_context(&mut self, context: KeyContext) {
        self.invalidator.debug_assert_paint();
        self.next_frame.dispatch_tree.set_key_context(context);
    }

    /// Sets the focus handle for the current element. This handle will be used to manage focus state
    /// and keyboard event dispatch for the element.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn set_focus_handle(&mut self, focus_handle: &FocusHandle, _: &App) {
        self.invalidator.debug_assert_prepaint();
        if focus_handle.is_focused(self) {
            self.next_frame.focus = Some(focus_handle.id);
        }
        self.next_frame.dispatch_tree.set_focus_id(focus_handle.id);
    }

    /// Olo: puts `focus_handle` in the window's tab order where this
    /// element is drawn (a div does this for a focus handle it tracks): for
    /// elements of their own that take the keyboard (BeOS's controls).
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn insert_tab_stop(&mut self, focus_handle: &FocusHandle) {
        self.invalidator.debug_assert_paint();
        self.next_frame.tab_stops.insert(focus_handle);
    }

    /// Sets the view id for the current element, which will be used to manage view caching.
    ///
    /// This method should only be called as part of element prepaint. We plan on removing this
    /// method eventually when we solve some issues that require us to construct editor elements
    /// directly instead of always using editors via views.
    pub fn set_view_id(&mut self, view_id: EntityId) {
        self.invalidator.debug_assert_prepaint();
        self.next_frame.dispatch_tree.set_view_id(view_id);
    }

    /// Get the entity ID for the currently rendering view
    pub fn current_view(&self) -> EntityId {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.rendered_entity_stack.last().copied().unwrap()
    }

    pub(crate) fn with_rendered_view<R>(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.rendered_entity_stack.push(id);
        let result = f(self);
        self.rendered_entity_stack.pop();
        result
    }

    /// Executes the provided function with the specified image cache.
    pub fn with_image_cache<F, R>(&mut self, image_cache: Option<AnyImageCache>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(image_cache) = image_cache {
            self.image_cache_stack.push(image_cache);
            let result = f(self);
            self.image_cache_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Sets an input handler, such as [`ElementInputHandler`][element_input_handler], which interfaces with the
    /// platform to receive textual input with proper integration with concerns such
    /// as IME interactions. This handler will be active for the upcoming frame until the following frame is
    /// rendered.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// [element_input_handler]: crate::ElementInputHandler
    pub fn handle_input(
        &mut self,
        focus_handle: &FocusHandle,
        input_handler: impl InputHandler,
        cx: &App,
    ) {
        self.invalidator.debug_assert_paint();

        if focus_handle.is_focused(self) {
            let cx = self.to_async(cx);
            self.next_frame
                .input_handlers
                .push(Some(PlatformInputHandler::new(cx, Box::new(input_handler))));
        }
    }

    /// Register a mouse event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_mouse_event<Event: MouseEvent>(
        &mut self,
        mut handler: impl FnMut(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.mouse_listeners.push(Some(Box::new(
            move |event: &dyn Any, phase: DispatchPhase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref() {
                    handler(event, phase, window, cx)
                }
            },
        )));
    }

    /// Register a key event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_key_event<Event: KeyEvent>(
        &mut self,
        listener: impl Fn(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_key_event(Rc::new(
            move |event: &dyn Any, phase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref::<Event>() {
                    listener(event, phase, window, cx)
                }
            },
        ));
    }

    /// Register a modifiers changed event listener on the window for the next frame.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_modifiers_changed(
        &mut self,
        listener: impl Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_modifiers_changed(Rc::new(
            move |event: &ModifiersChangedEvent, window: &mut Window, cx: &mut App| {
                listener(event, window, cx)
            },
        ));
    }

    /// Register a listener to be called when the given focus handle or one of its descendants receives focus.
    /// This does not fire if the given focus handle - or one of its descendants - was previously focused.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_in(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if event.is_focus_in(focus_id) {
                    listener(window, cx);
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_out(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(FocusOutEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if let Some(blurred_id) = event.previous_focus_path.last().copied()
                    && event.is_focus_out(focus_id)
                {
                    let event = FocusOutEvent {
                        blurred: WeakFocusHandle {
                            id: blurred_id,
                            handles: Arc::downgrade(&cx.focus_handles),
                        },
                    };
                    listener(event, window, cx)
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    fn reset_cursor_style(&self, cx: &mut App) {
        // Set the cursor only if we're the active window.
        if self.is_window_hovered() {
            let style = self
                .rendered_frame
                .cursor_style(self)
                .unwrap_or(CursorStyle::Arrow);
            cx.platform.set_cursor_style(style);
        }
    }

    /// Dispatch a given keystroke as though the user had typed it.
    /// You can create a keystroke with Keystroke::parse("").
    pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
        let keystroke = keystroke.with_simulated_ime();
        let result = self.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
            }),
            cx,
        );
        if !result.propagate {
            return true;
        }

        if let Some(input) = keystroke.key_char
            && let Some(mut input_handler) = self.platform_window.take_input_handler()
        {
            input_handler.dispatch_input(&input, self, cx);
            self.platform_window.set_input_handler(input_handler);
            return true;
        }

        false
    }

    /// Return a key binding string for an action, to display in the UI. Uses the highest precedence
    /// binding for the action (last binding added to the keymap).
    pub fn keystroke_text_for(&self, action: &dyn Action) -> String {
        self.highest_precedence_binding_for_action(action)
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| action.name().to_string())
    }

    /// Dispatch a mouse or keyboard event on the window.
    #[profiling::function]
    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> DispatchEventResult {
        self.last_input_timestamp.set(Instant::now());
        // Handlers may set this to false by calling `stop_propagation`.
        cx.propagate_event = true;
        // Handlers may set this to true by calling `prevent_default`.
        self.default_prevented = false;

        let event = match event {
            // Track the mouse position with our own state, since accessing the platform
            // API for the mouse position can only occur on the main thread.
            PlatformInput::MouseMove(mouse_move) => {
                self.mouse_position = mouse_move.position;
                self.modifiers = mouse_move.modifiers;
                PlatformInput::MouseMove(mouse_move)
            }
            PlatformInput::MouseDown(mouse_down) => {
                self.mouse_position = mouse_down.position;
                self.modifiers = mouse_down.modifiers;
                PlatformInput::MouseDown(mouse_down)
            }
            PlatformInput::MouseUp(mouse_up) => {
                self.mouse_position = mouse_up.position;
                self.modifiers = mouse_up.modifiers;
                PlatformInput::MouseUp(mouse_up)
            }
            PlatformInput::MouseExited(mouse_exited) => {
                self.modifiers = mouse_exited.modifiers;
                // Olo: gone, the pointer is over nothing of the window, so
                // nothing reads as hovered (a tab or button lit under it
                // stayed lit, the pointer last seen there), and what was
                // is drawn again.
                self.mouse_position = point(px(-1.0e6), px(-1.0e6));
                if !self.mouse_hit_test.ids.is_empty() {
                    self.refresh();
                }
                // Olo: and, no button held, a move to nowhere, so what
                // follows the pointer by its moves sees it go (a key,
                // chart or row lit under it stayed lit when it left the
                // window from there: DeskCalc's keys, a graph's readout).
                // Held, it's a drag's, left as it is.
                if mouse_exited.pressed_button.is_none() {
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: self.mouse_position,
                        pressed_button: None,
                        modifiers: mouse_exited.modifiers,
                    })
                } else {
                    PlatformInput::MouseExited(mouse_exited)
                }
            }
            PlatformInput::ModifiersChanged(modifiers_changed) => {
                self.modifiers = modifiers_changed.modifiers;
                self.capslock = modifiers_changed.capslock;
                PlatformInput::ModifiersChanged(modifiers_changed)
            }
            PlatformInput::ScrollWheel(scroll_wheel) => {
                self.mouse_position = scroll_wheel.position;
                self.modifiers = scroll_wheel.modifiers;
                PlatformInput::ScrollWheel(scroll_wheel)
            }
            PlatformInput::Pinch(pinch) => {
                self.mouse_position = pinch.position;
                PlatformInput::Pinch(pinch)
            }
            // Translate dragging and dropping of external files from the operating system
            // to internal drag and drop events.
            PlatformInput::FileDrop(file_drop) => match file_drop {
                FileDropEvent::Entered { position, paths } => {
                    self.mouse_position = position;
                    if cx.active_drag.is_none() {
                        cx.active_drag = Some(AnyDrag {
                            value: Arc::new(paths.clone()),
                            view: cx.new(|_| paths).into(),
                            cursor_offset: position,
                            cursor_style: None,
                        });
                    }
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Pending { position } => {
                    self.mouse_position = position;
                    // (Its parts take it again where it is now.)
                    if self.drop_offer.is_some() {
                        self.refresh();
                    }
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Submit { position } => {
                    cx.activate(true);
                    self.mouse_position = position;
                    self.drop_offer = None;
                    self.drop_accept_sent = None;
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                }
                FileDropEvent::Exited => {
                    cx.active_drag.take();
                    self.drop_offer = None;
                    self.drop_accept_sent = None;
                    self.refresh();
                    PlatformInput::FileDrop(FileDropEvent::Exited)
                }
                // Olo: what's dragged over it, for its parts to take.
                FileDropEvent::Offered(offer) => {
                    self.drop_offer = Some(offer);
                    self.drop_accept_sent = None;
                    self.refresh();
                    return DispatchEventResult::default();
                }
                FileDropEvent::Dropped(dropped) => {
                    self.drop_offer = None;
                    self.drop_accept_sent = None;
                    self.refresh();
                    if let Some(handler) = self.drop_handler.clone() {
                        cx.activate(true);
                        (handler.borrow_mut())(dropped, self, cx);
                    }
                    return DispatchEventResult::default();
                }
            },
            PlatformInput::KeyDown(_) | PlatformInput::KeyUp(_) => event,
        };

        if let Some(any_mouse_event) = event.mouse_event() {
            self.dispatch_mouse_event(any_mouse_event, cx);
        } else if let Some(any_key_event) = event.keyboard_event() {
            self.dispatch_key_event(any_key_event, cx);
        }

        DispatchEventResult {
            propagate: cx.propagate_event,
            default_prevented: self.default_prevented,
        }
    }

    fn dispatch_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        let hit_test = self.rendered_frame.hit_test(self.mouse_position());
        if hit_test != self.mouse_hit_test {
            self.mouse_hit_test = hit_test;
            self.reset_cursor_style(cx);
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.is_inspector_picking(cx) {
            self.handle_inspector_mouse_event(event, cx);
            // When inspector is picking, all other mouse handling is skipped.
            return;
        }

        let mut mouse_listeners = mem::take(&mut self.rendered_frame.mouse_listeners);

        // Capture phase, events bubble from back to front. Handlers for this phase are used for
        // special purposes, such as detecting events outside of a given Bounds.
        for listener in &mut mouse_listeners {
            let listener = listener.as_mut().unwrap();
            listener(event, DispatchPhase::Capture, self, cx);
            if !cx.propagate_event {
                break;
            }
        }

        // Bubble phase, where most normal handlers do their work.
        if cx.propagate_event {
            for listener in mouse_listeners.iter_mut().rev() {
                let listener = listener.as_mut().unwrap();
                listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    break;
                }
            }
        }

        self.rendered_frame.mouse_listeners = mouse_listeners;

        if cx.has_active_drag() {
            if event.is::<MouseMoveEvent>() {
                // If this was a mouse move event, redraw the window so that the
                // active drag can follow the mouse cursor.
                self.refresh();
            } else if event.is::<MouseUpEvent>() {
                // If this was a mouse up event, cancel the active drag and redraw
                // the window.
                cx.active_drag = None;
                self.refresh();
            }
        }
    }

    fn dispatch_key_event(&mut self, event: &dyn Any, cx: &mut App) {
        if self.invalidator.is_dirty() {
            self.draw(cx).clear();
        }

        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        let mut keystroke: Option<Keystroke> = None;

        if let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() {
            if event.modifiers.number_of_modifiers() == 0
                && self.pending_modifier.modifiers.number_of_modifiers() == 1
                && !self.pending_modifier.saw_keystroke
            {
                let key = match self.pending_modifier.modifiers {
                    modifiers if modifiers.shift => Some("shift"),
                    modifiers if modifiers.control => Some("control"),
                    modifiers if modifiers.alt => Some("alt"),
                    modifiers if modifiers.platform => Some("platform"),
                    modifiers if modifiers.function => Some("function"),
                    _ => None,
                };
                if let Some(key) = key {
                    keystroke = Some(Keystroke {
                        key: key.to_string(),
                        key_char: None,
                        modifiers: Modifiers::default(),
                    });
                }
            }

            if self.pending_modifier.modifiers.number_of_modifiers() == 0
                && event.modifiers.number_of_modifiers() == 1
            {
                self.pending_modifier.saw_keystroke = false
            }
            self.pending_modifier.modifiers = event.modifiers
        } else if let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() {
            self.pending_modifier.saw_keystroke = true;
            keystroke = Some(key_down_event.keystroke.clone());
        }

        let Some(keystroke) = keystroke else {
            self.finish_dispatch_key_event(event, dispatch_path, self.context_stack(), cx);
            return;
        };

        cx.propagate_event = true;
        self.dispatch_keystroke_interceptors(event, self.context_stack(), cx);
        if !cx.propagate_event {
            self.finish_dispatch_key_event(event, dispatch_path, self.context_stack(), cx);
            return;
        }

        let mut currently_pending = self.pending_input.take().unwrap_or_default();
        if currently_pending.focus.is_some() && currently_pending.focus != self.focus {
            currently_pending = PendingInput::default();
        }

        let match_result = self.rendered_frame.dispatch_tree.dispatch_key(
            currently_pending.keystrokes,
            keystroke,
            &dispatch_path,
        );

        if !match_result.to_replay.is_empty() {
            self.replay_pending_input(match_result.to_replay, cx);
            cx.propagate_event = true;
        }

        if !match_result.pending.is_empty() {
            currently_pending.keystrokes = match_result.pending;
            currently_pending.focus = self.focus;
            currently_pending.timer = Some(self.spawn(cx, async move |cx| {
                cx.background_executor.timer(Duration::from_secs(1)).await;
                cx.update(move |window, cx| {
                    let Some(currently_pending) = window
                        .pending_input
                        .take()
                        .filter(|pending| pending.focus == window.focus)
                    else {
                        return;
                    };

                    let node_id = window.focus_node_id_in_rendered_frame(window.focus);
                    let dispatch_path = window.rendered_frame.dispatch_tree.dispatch_path(node_id);

                    let to_replay = window
                        .rendered_frame
                        .dispatch_tree
                        .flush_dispatch(currently_pending.keystrokes, &dispatch_path);

                    window.pending_input_changed(cx);
                    window.replay_pending_input(to_replay, cx)
                })
                .ok();
            }));
            self.pending_input = Some(currently_pending);
            self.pending_input_changed(cx);
            cx.propagate_event = false;
            return;
        }

        for binding in match_result.bindings {
            self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
            if !cx.propagate_event {
                self.dispatch_keystroke_observers(
                    event,
                    Some(binding.action),
                    match_result.context_stack,
                    cx,
                );
                self.pending_input_changed(cx);
                return;
            }
        }

        self.finish_dispatch_key_event(event, dispatch_path, match_result.context_stack, cx);
        self.pending_input_changed(cx);
    }

    fn finish_dispatch_key_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: SmallVec<[DispatchNodeId; 32]>,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        self.dispatch_key_down_up_event(event, &dispatch_path, cx);
        if !cx.propagate_event {
            return;
        }

        self.dispatch_modifiers_changed_event(event, &dispatch_path, cx);
        if !cx.propagate_event {
            return;
        }

        self.dispatch_keystroke_observers(event, None, context_stack, cx);
    }

    fn pending_input_changed(&mut self, cx: &mut App) {
        self.pending_input_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    fn dispatch_key_down_up_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        // Capture phase
        for node_id in dispatch_path {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);

            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Capture, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }

        // Bubble phase
        for node_id in dispatch_path.iter().rev() {
            // Handle low level key events
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    fn dispatch_modifiers_changed_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() else {
            return;
        };
        for node_id in dispatch_path.iter().rev() {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for listener in node.modifiers_changed_listeners.clone() {
                listener(event, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    /// Determine whether a potential multi-stroke key binding is in progress on this window.
    pub fn has_pending_keystrokes(&self) -> bool {
        self.pending_input.is_some()
    }

    pub(crate) fn clear_pending_keystrokes(&mut self) {
        self.pending_input.take();
    }

    /// Returns the currently pending input keystrokes that might result in a multi-stroke key binding.
    pub fn pending_input_keystrokes(&self) -> Option<&[Keystroke]> {
        self.pending_input
            .as_ref()
            .map(|pending_input| pending_input.keystrokes.as_slice())
    }

    fn replay_pending_input(&mut self, replays: SmallVec<[Replay; 1]>, cx: &mut App) {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        'replay: for replay in replays {
            let event = KeyDownEvent {
                keystroke: replay.keystroke.clone(),
                is_held: false,
            };

            cx.propagate_event = true;
            for binding in replay.bindings {
                self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
                if !cx.propagate_event {
                    self.dispatch_keystroke_observers(
                        &event,
                        Some(binding.action),
                        Vec::default(),
                        cx,
                    );
                    continue 'replay;
                }
            }

            self.dispatch_key_down_up_event(&event, &dispatch_path, cx);
            if !cx.propagate_event {
                continue 'replay;
            }
            if let Some(input) = replay.keystroke.key_char.as_ref().cloned()
                && let Some(mut input_handler) = self.platform_window.take_input_handler()
            {
                input_handler.dispatch_input(&input, self, cx);
                self.platform_window.set_input_handler(input_handler)
            }
        }
    }

    fn focus_node_id_in_rendered_frame(&self, focus_id: Option<FocusId>) -> DispatchNodeId {
        focus_id
            .and_then(|focus_id| {
                self.rendered_frame
                    .dispatch_tree
                    .focusable_node_id(focus_id)
            })
            .unwrap_or_else(|| self.rendered_frame.dispatch_tree.root_node_id())
    }

    fn dispatch_action_on_node(
        &mut self,
        node_id: DispatchNodeId,
        action: &dyn Action,
        cx: &mut App,
    ) {
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        // Capture phase for global actions.
        cx.propagate_event = true;
        if let Some(mut global_listeners) = cx
            .global_action_listeners
            .remove(&action.as_any().type_id())
        {
            for listener in &global_listeners {
                listener(action.as_any(), DispatchPhase::Capture, cx);
                if !cx.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                cx.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            cx.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }

        if !cx.propagate_event {
            return;
        }

        // Capture phase for window actions.
        for node_id in &dispatch_path {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for DispatchActionListener {
                action_type,
                listener,
            } in node.action_listeners.clone()
            {
                let any_action = action.as_any();
                if action_type == any_action.type_id() {
                    listener(any_action, DispatchPhase::Capture, self, cx);

                    if !cx.propagate_event {
                        return;
                    }
                }
            }
        }

        // Bubble phase for window actions.
        for node_id in dispatch_path.iter().rev() {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for DispatchActionListener {
                action_type,
                listener,
            } in node.action_listeners.clone()
            {
                let any_action = action.as_any();
                if action_type == any_action.type_id() {
                    cx.propagate_event = false; // Actions stop propagation by default during the bubble phase
                    listener(any_action, DispatchPhase::Bubble, self, cx);

                    if !cx.propagate_event {
                        return;
                    }
                }
            }
        }

        // Bubble phase for global actions.
        if let Some(mut global_listeners) = cx
            .global_action_listeners
            .remove(&action.as_any().type_id())
        {
            for listener in global_listeners.iter().rev() {
                cx.propagate_event = false; // Actions stop propagation by default during the bubble phase

                listener(action.as_any(), DispatchPhase::Bubble, cx);
                if !cx.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                cx.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            cx.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }
    }

    /// Register the given handler to be invoked whenever the global of the given type
    /// is updated.
    pub fn observe_global<G: Global>(
        &mut self,
        cx: &mut App,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let window_handle = self.handle;
        let (subscription, activate) = cx.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| f(window, cx))
                    .is_ok()
            }),
        );
        cx.defer(move |_| activate());
        subscription
    }

    /// Focus the current window and bring it to the foreground at the platform level.
    pub fn activate_window(&self) {
        self.platform_window.activate();
    }

    /// Olo: keeps the screen from dimming, locking or sleeping while the
    /// window shows (a playing video), until called with `false` or the
    /// window closes.
    pub fn set_keep_awake(&self, on: bool) {
        self.platform_window.set_keep_awake(on);
    }

    /// Olo: shows external picture `key` ([`Window::set_external_buffer`])
    /// at `bounds` this frame: drawn by the compositor itself, over what
    /// the window draws there, the pointer passing through it to the
    /// window; moved and resized with the window's frames, while each new
    /// picture shows as it comes, without one. A picture not placed in a
    /// frame is hidden. Shown at whole logical pixels (subsurfaces can't
    /// be placed finer): draw round it at [`Window::external_bounds`].
    pub fn place_external(&mut self, key: u64, bounds: Bounds<Pixels>) {
        self.had_externals.set(true);
        self.next_frame
            .paint_records
            .push((Rc::new(ExternalPlace { key, bounds }), Point::default()));
    }

    /// Olo: where an external picture placed at `bounds` shows, to the
    /// device pixel: its corners at whole logical pixels, then at the
    /// device pixels the compositor rounds them to.
    pub fn external_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let s = self.scale_factor();
        let (x, y) = (bounds.origin.x.0.round(), bounds.origin.y.0.round());
        let (w, h) = (
            bounds.size.width.0.round().max(1.0),
            bounds.size.height.0.round().max(1.0),
        );
        let d = |v: f32| px((v * s).round() / s);
        Bounds::from_corners(point(d(x), d(y)), point(d(x + w), d(y + h)))
    }

    /// Olo: the picture external picture `key` shows from now on (another
    /// program's dmabuf, made into a buffer once by its id); the one it
    /// replaces is released (`ExternalBuffer::released`) once the
    /// compositor is done with it. None lets `key` go with its buffers.
    /// Shows where [`Window::place_external`] placed it, at once.
    pub fn set_external_buffer(&self, key: u64, buffer: Option<crate::ExternalBuffer>) {
        self.platform_window.set_external_buffer(key, buffer);
    }

    /// Olo: a handle another program's window can name to open over this
    /// one (xdg-foreign, `WindowOptions::foreign_parent` there: a file
    /// panel's look in the Viewer); `done` gets None if the compositor
    /// can't export windows. The window stays exported until it closes.
    pub fn export_handle(&self, done: impl FnOnce(Option<String>) + 'static) {
        self.platform_window.export_handle(Box::new(done));
    }

    /// Olo: an activation token made with this window and the last press
    /// in it, for another program to take the focus with (a
    /// notification's app, as its action is invoked); `done` gets None
    /// if the compositor has no activation protocol.
    pub fn activation_token(&self, done: impl FnOnce(Option<String>) + 'static) {
        self.platform_window.activation_token(Box::new(done));
    }

    /// Olo: whether this layer surface takes the keyboard, changed after
    /// it opened (notifications take it while a reply field shows).
    pub fn set_layer_keyboard(&self, keyboard: crate::LayerKeyboard) {
        self.platform_window.set_layer_keyboard(keyboard);
    }

    /// Olo: this layer surface's margins (top, right, bottom, left, in
    /// points), changed after it opened: it moves (a recording's camera
    /// bubble, dragged).
    pub fn set_layer_margin(&self, margin: (i32, i32, i32, i32)) {
        self.platform_window.set_layer_margin(margin);
    }

    /// Olo: draws `font`'s printable characters at `font_size` for this
    /// window ahead of need, off its thread, so the frame that shows
    /// them only copies them (drawing a glyph the first time costs a
    /// frame ~12 µs). GPUI does it itself for a face and size once a few
    /// dozen of its glyphs have been drawn (running text); an app asks for
    /// those it will draw soon (a conversation's text, before a chat is
    /// opened).
    /// The window's next frame, and each after it moves to a display
    /// with another scale or text rendering, has them drawn for that.
    pub fn warm_glyphs(&mut self, font: &crate::Font, font_size: Pixels) {
        let font_id = self.text_system().resolve_font(font);
        self.warm.push((font_id, font_size));
        self.warmed_for = None;
        self.warm_for_display();
    }

    fn warm_for_display(&mut self) {
        let now = (self.scale_factor(), self.platform_window.text_rendering());
        if self.warmed_for == Some(now) {
            return;
        }
        self.warmed_for = Some(now);
        for &(font_id, font_size) in &self.warm {
            self.text_system.warm(font_id, font_size, now.0, now.1);
        }
        // (At another scale: their glyphs drawn again for it.)
        for (font_id, font_size, texts, scale) in &mut self.warm_texts {
            if *scale != now.0 {
                *scale = now.0;
                self.text_system
                    .warm_text(*font_id, *font_size, now.0, texts.clone());
            }
        }
    }

    /// Olo: draws the colour glyphs (emoji) of `texts` in `font` at
    /// `font_size` ahead of need, off the UI thread, in the order given,
    /// so the frames that show them only copy them; and again for each
    /// display with another scale the window moves to. They're kept for
    /// every window of the app after (up to 32 MB of them;
    /// [`crate::TextSystem::forget_kept_glyphs`] lets go): a picker of
    /// emoji opened again and again, each time a new window.
    pub fn warm_text(
        &mut self,
        font: &crate::Font,
        font_size: Pixels,
        texts: impl Into<Arc<[SharedString]>>,
    ) {
        let font_id = self.text_system().resolve_font(font);
        let texts = texts.into();
        let scale = self.scale_factor();
        self.text_system
            .warm_text(font_id, font_size, scale, texts.clone());
        self.warm_texts.push((font_id, font_size, texts, scale));
    }

    /// Bring the window forward with an activation token from another
    /// program (a notification clicked, a launcher), which lets it take
    /// the focus where [`activate_window`](Self::activate_window) may only
    /// ask for attention.
    pub fn activate_window_with_token(&self, token: &str) {
        self.platform_window.activate_with_token(token);
    }

    /// Minimize the current window at the platform level.
    pub fn minimize_window(&self) {
        self.platform_window.minimize();
    }

    /// Toggle full screen status on the current window at the platform level.
    pub fn toggle_fullscreen(&self) {
        self.platform_window.toggle_fullscreen();
    }

    /// Updates the IME panel position suggestions for languages like japanese, chinese.
    pub fn invalidate_character_coordinates(&self) {
        self.on_next_frame(|window, cx| {
            if let Some(mut input_handler) = window.platform_window.take_input_handler() {
                if let Some(bounds) = input_handler.selected_bounds(window, cx) {
                    window.platform_window.update_ime_position(bounds);
                }
                window.platform_window.set_input_handler(input_handler);
            }
        });
    }

    /// Present a platform dialog.
    /// The provided message will be presented, along with buttons for each answer.
    /// When a button is clicked, the returned Receiver will receive the index of the clicked button.
    pub fn prompt<T>(
        &mut self,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[T],
        cx: &mut App,
    ) -> oneshot::Receiver<usize>
    where
        T: Clone + Into<PromptButton>,
    {
        let prompt_builder = cx.prompt_builder.take();
        let Some(prompt_builder) = prompt_builder else {
            unreachable!("Re-entrant window prompting is not supported by GPUI");
        };

        let answers = answers
            .iter()
            .map(|answer| answer.clone().into())
            .collect::<Vec<_>>();

        let receiver = match &prompt_builder {
            PromptBuilder::Default => self
                .platform_window
                .prompt(level, message, detail, &answers)
                .unwrap_or_else(|| {
                    self.build_custom_prompt(&prompt_builder, level, message, detail, &answers, cx)
                }),
            PromptBuilder::Custom(_) => {
                self.build_custom_prompt(&prompt_builder, level, message, detail, &answers, cx)
            }
        };

        cx.prompt_builder = Some(prompt_builder);

        receiver
    }

    fn build_custom_prompt(
        &mut self,
        prompt_builder: &PromptBuilder,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
        cx: &mut App,
    ) -> oneshot::Receiver<usize> {
        let (sender, receiver) = oneshot::channel();
        let handle = PromptHandle::new(sender);
        let handle = (prompt_builder)(level, message, detail, answers, handle, self, cx);
        self.prompt = Some(handle);
        receiver
    }

    /// Returns the current context stack.
    pub fn context_stack(&self) -> Vec<KeyContext> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree
            .dispatch_path(node_id)
            .iter()
            .filter_map(move |&node_id| dispatch_tree.node(node_id).context.clone())
            .collect()
    }

    /// Returns all available actions for the focused element.
    pub fn available_actions(&self, cx: &App) -> Vec<Box<dyn Action>> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let mut actions = self.rendered_frame.dispatch_tree.available_actions(node_id);
        for action_type in cx.global_action_listeners.keys() {
            if let Err(ix) = actions.binary_search_by_key(action_type, |a| a.as_any().type_id()) {
                let action = cx.actions.build_action_type(action_type).ok();
                if let Some(action) = action {
                    actions.insert(ix, action);
                }
            }
        }
        actions
    }

    /// Returns key bindings that invoke an action on the currently focused element. Bindings are
    /// returned in the order they were added. For display, the last binding should take precedence.
    pub fn bindings_for_action(&self, action: &dyn Action) -> Vec<KeyBinding> {
        self.rendered_frame
            .dispatch_tree
            .bindings_for_action(action, &self.rendered_frame.dispatch_tree.context_stack)
    }

    /// Returns the highest precedence key binding that invokes an action on the currently focused
    /// element. This is more efficient than getting the last result of `bindings_for_action`.
    pub fn highest_precedence_binding_for_action(&self, action: &dyn Action) -> Option<KeyBinding> {
        self.rendered_frame
            .dispatch_tree
            .highest_precedence_binding_for_action(
                action,
                &self.rendered_frame.dispatch_tree.context_stack,
            )
    }

    /// Returns the key bindings for an action in a context.
    pub fn bindings_for_action_in_context(
        &self,
        action: &dyn Action,
        context: KeyContext,
    ) -> Vec<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree.bindings_for_action(action, &[context])
    }

    /// Returns the highest precedence key binding for an action in a context. This is more
    /// efficient than getting the last result of `bindings_for_action_in_context`.
    pub fn highest_precedence_binding_for_action_in_context(
        &self,
        action: &dyn Action,
        context: KeyContext,
    ) -> Option<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree.highest_precedence_binding_for_action(action, &[context])
    }

    /// Returns any bindings that would invoke an action on the given focus handle if it were
    /// focused. Bindings are returned in the order they were added. For display, the last binding
    /// should take precedence.
    pub fn bindings_for_action_in(
        &self,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> Vec<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let Some(context_stack) = self.context_stack_for_focus_handle(focus_handle) else {
            return vec![];
        };
        dispatch_tree.bindings_for_action(action, &context_stack)
    }

    /// Returns the highest precedence key binding that would invoke an action on the given focus
    /// handle if it were focused. This is more efficient than getting the last result of
    /// `bindings_for_action_in`.
    pub fn highest_precedence_binding_for_action_in(
        &self,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> Option<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let context_stack = self.context_stack_for_focus_handle(focus_handle)?;
        dispatch_tree.highest_precedence_binding_for_action(action, &context_stack)
    }

    fn context_stack_for_focus_handle(
        &self,
        focus_handle: &FocusHandle,
    ) -> Option<Vec<KeyContext>> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let node_id = dispatch_tree.focusable_node_id(focus_handle.id)?;
        let context_stack: Vec<_> = dispatch_tree
            .dispatch_path(node_id)
            .into_iter()
            .filter_map(|node_id| dispatch_tree.node(node_id).context.clone())
            .collect();
        Some(context_stack)
    }

    /// Returns a generic event listener that invokes the given listener with the view and context associated with the given view handle.
    pub fn listener_for<V: Render, E>(
        &self,
        view: &Entity<V>,
        f: impl Fn(&mut V, &E, &mut Window, &mut Context<V>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = view.downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx)).ok();
        }
    }

    /// Returns a generic handler that invokes the given handler with the view and context associated with the given view handle.
    pub fn handler_for<E: 'static, Callback: Fn(&mut E, &mut Window, &mut Context<E>) + 'static>(
        &self,
        entity: &Entity<E>,
        f: Callback,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let entity = entity.downgrade();
        move |window: &mut Window, cx: &mut App| {
            entity.update(cx, |entity, cx| f(entity, window, cx)).ok();
        }
    }

    /// Register a callback that can interrupt the closing of the current window based the returned boolean.
    /// If the callback returns false, the window won't be closed.
    pub fn on_window_should_close(
        &self,
        cx: &App,
        f: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) {
        let mut cx = self.to_async(cx);
        self.platform_window.on_should_close(Box::new(move || {
            cx.update(|window, cx| f(window, cx)).unwrap_or(true)
        }))
    }

    /// Register an action listener on the window for the next frame. The type of action
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using action handlers on elements unless you have
    /// a specific need to register a global listener.
    pub fn on_action(
        &mut self,
        action_type: TypeId,
        listener: impl Fn(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.next_frame
            .dispatch_tree
            .on_action(action_type, Rc::new(listener));
    }

    /// Register an action listener on the window for the next frame if the condition is true.
    /// The type of action is determined by the first parameter of the given listener.
    /// When the next frame is rendered the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using action handlers on elements unless you have
    /// a specific need to register a global listener.
    pub fn on_action_when(
        &mut self,
        condition: bool,
        action_type: TypeId,
        listener: impl Fn(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        if condition {
            self.next_frame
                .dispatch_tree
                .on_action(action_type, Rc::new(listener));
        }
    }

    /// Read information about the GPU backing this window.
    /// Currently returns None on Mac and Windows.
    pub fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.platform_window.gpu_specs()
    }

    /// Perform titlebar double-click action.
    /// This is macOS specific.
    pub fn titlebar_double_click(&self) {
        self.platform_window.titlebar_double_click();
    }

    /// Gets the window's title at the platform level.
    /// This is macOS specific.
    pub fn window_title(&self) -> String {
        self.platform_window.get_title()
    }

    /// Returns a list of all tabbed windows and their titles.
    /// This is macOS specific.
    pub fn tabbed_windows(&self) -> Option<Vec<SystemWindowTab>> {
        self.platform_window.tabbed_windows()
    }

    /// Returns the tab bar visibility.
    /// This is macOS specific.
    pub fn tab_bar_visible(&self) -> bool {
        self.platform_window.tab_bar_visible()
    }

    /// Merges all open windows into a single tabbed window.
    /// This is macOS specific.
    pub fn merge_all_windows(&self) {
        self.platform_window.merge_all_windows()
    }

    /// Moves the tab to a new containing window.
    /// This is macOS specific.
    pub fn move_tab_to_new_window(&self) {
        self.platform_window.move_tab_to_new_window()
    }

    /// Shows or hides the window tab overview.
    /// This is macOS specific.
    pub fn toggle_window_tab_overview(&self) {
        self.platform_window.toggle_window_tab_overview()
    }

    /// Sets the tabbing identifier for the window.
    /// This is macOS specific.
    pub fn set_tabbing_identifier(&self, tabbing_identifier: Option<String>) {
        self.platform_window
            .set_tabbing_identifier(tabbing_identifier)
    }

    /// Toggles the inspector mode on this window.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn toggle_inspector(&mut self, cx: &mut App) {
        self.inspector = match self.inspector {
            None => Some(cx.new(|_| Inspector::new())),
            Some(_) => None,
        };
        self.refresh();
    }

    /// Returns true if the window is in inspector mode.
    pub fn is_inspector_picking(&self, _cx: &App) -> bool {
        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            if let Some(inspector) = &self.inspector {
                return inspector.read(_cx).is_picking();
            }
        }
        false
    }

    /// Executes the provided function with mutable access to an inspector state.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn with_inspector_state<T: 'static, R>(
        &mut self,
        _inspector_id: Option<&crate::InspectorElementId>,
        cx: &mut App,
        f: impl FnOnce(&mut Option<T>, &mut Self) -> R,
    ) -> R {
        if let Some(inspector_id) = _inspector_id
            && let Some(inspector) = &self.inspector
        {
            let inspector = inspector.clone();
            let active_element_id = inspector.read(cx).active_element_id();
            if Some(inspector_id) == active_element_id {
                return inspector.update(cx, |inspector, _cx| {
                    inspector.with_active_element_state(self, f)
                });
            }
        }
        f(&mut None, self)
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) fn build_inspector_element_id(
        &mut self,
        path: crate::InspectorElementPath,
    ) -> crate::InspectorElementId {
        self.invalidator.debug_assert_paint_or_prepaint();
        let path = Rc::new(path);
        let next_instance_id = self
            .next_frame
            .next_inspector_instance_ids
            .entry(path.clone())
            .or_insert(0);
        let instance_id = *next_instance_id;
        *next_instance_id += 1;
        crate::InspectorElementId { path, instance_id }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn prepaint_inspector(&mut self, inspector_width: Pixels, cx: &mut App) -> Option<AnyElement> {
        if let Some(inspector) = self.inspector.take() {
            let mut inspector_element = AnyView::from(inspector.clone()).into_any_element();
            inspector_element.prepaint_as_root(
                point(self.viewport_size.width - inspector_width, px(0.0)),
                size(inspector_width, self.viewport_size.height).into(),
                self,
                cx,
            );
            self.inspector = Some(inspector);
            Some(inspector_element)
        } else {
            None
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn paint_inspector(&mut self, mut inspector_element: Option<AnyElement>, cx: &mut App) {
        if let Some(mut inspector_element) = inspector_element {
            inspector_element.paint(self, cx);
        };
    }

    /// Registers a hitbox that can be used for inspector picking mode, allowing users to select and
    /// inspect UI elements by clicking on them.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn insert_inspector_hitbox(
        &mut self,
        hitbox_id: HitboxId,
        inspector_id: Option<&crate::InspectorElementId>,
        cx: &App,
    ) {
        self.invalidator.debug_assert_paint_or_prepaint();
        if !self.is_inspector_picking(cx) {
            return;
        }
        if let Some(inspector_id) = inspector_id {
            self.next_frame
                .inspector_hitboxes
                .insert(hitbox_id, inspector_id.clone());
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn paint_inspector_hitbox(&mut self, cx: &App) {
        if let Some(inspector) = self.inspector.as_ref() {
            let inspector = inspector.read(cx);
            if let Some((hitbox_id, _)) = self.hovered_inspector_hitbox(inspector, &self.next_frame)
                && let Some(hitbox) = self
                    .next_frame
                    .hitboxes
                    .iter()
                    .find(|hitbox| hitbox.id == hitbox_id)
            {
                self.paint_quad(crate::fill(hitbox.bounds, crate::rgba(0x61afef4d)));
            }
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn handle_inspector_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        let Some(inspector) = self.inspector.clone() else {
            return;
        };
        if event.downcast_ref::<MouseMoveEvent>().is_some() {
            inspector.update(cx, |inspector, _cx| {
                if let Some((_, inspector_id)) =
                    self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                {
                    inspector.hover(inspector_id, self);
                }
            });
        } else if event.downcast_ref::<crate::MouseDownEvent>().is_some() {
            inspector.update(cx, |inspector, _cx| {
                if let Some((_, inspector_id)) =
                    self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                {
                    inspector.select(inspector_id, self);
                }
            });
        } else if let Some(event) = event.downcast_ref::<crate::ScrollWheelEvent>() {
            // This should be kept in sync with SCROLL_LINES in x11 platform.
            const SCROLL_LINES: f32 = 3.0;
            const SCROLL_PIXELS_PER_LAYER: f32 = 36.0;
            let delta_y = event
                .delta
                .pixel_delta(px(SCROLL_PIXELS_PER_LAYER / SCROLL_LINES))
                .y;
            if let Some(inspector) = self.inspector.clone() {
                inspector.update(cx, |inspector, _cx| {
                    if let Some(depth) = inspector.pick_depth.as_mut() {
                        *depth += f32::from(delta_y) / SCROLL_PIXELS_PER_LAYER;
                        let max_depth = self.mouse_hit_test.ids.len() as f32 - 0.5;
                        if *depth < 0.0 {
                            *depth = 0.0;
                        } else if *depth > max_depth {
                            *depth = max_depth;
                        }
                        if let Some((_, inspector_id)) =
                            self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                        {
                            inspector.set_active_element_id(inspector_id, self);
                        }
                    }
                });
            }
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn hovered_inspector_hitbox(
        &self,
        inspector: &Inspector,
        frame: &Frame,
    ) -> Option<(HitboxId, crate::InspectorElementId)> {
        if let Some(pick_depth) = inspector.pick_depth {
            let depth = (pick_depth as i64).try_into().unwrap_or(0);
            let max_skipped = self.mouse_hit_test.ids.len().saturating_sub(1);
            let skip_count = (depth as usize).min(max_skipped);
            for hitbox_id in self.mouse_hit_test.ids.iter().skip(skip_count) {
                if let Some(inspector_id) = frame.inspector_hitboxes.get(hitbox_id) {
                    return Some((*hitbox_id, inspector_id.clone()));
                }
            }
        }
        None
    }

    /// For testing: set the current modifier keys state.
    /// This does not generate any events.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }
}

// #[derive(Clone, Copy, Eq, PartialEq, Hash)]
slotmap::new_key_type! {
    /// A unique identifier for a window.
    pub struct WindowId;
}

impl WindowId {
    /// Converts this window ID to a `u64`.
    pub fn as_u64(&self) -> u64 {
        self.0.as_ffi()
    }
}

impl From<u64> for WindowId {
    fn from(value: u64) -> Self {
        WindowId(slotmap::KeyData::from_ffi(value))
    }
}

/// A handle to a window with a specific root view type.
/// Note that this does not keep the window alive on its own.
#[derive(Deref, DerefMut)]
pub struct WindowHandle<V> {
    #[deref]
    #[deref_mut]
    pub(crate) any_handle: AnyWindowHandle,
    state_type: PhantomData<fn(V) -> V>,
}

impl<V> Debug for WindowHandle<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowHandle")
            .field("any_handle", &self.any_handle.id.as_u64())
            .finish()
    }
}

impl<V: 'static + Render> WindowHandle<V> {
    /// Creates a new handle from a window ID.
    /// This does not check if the root type of the window is `V`.
    pub fn new(id: WindowId) -> Self {
        WindowHandle {
            any_handle: AnyWindowHandle {
                id,
                state_type: TypeId::of::<V>(),
            },
            state_type: PhantomData,
        }
    }

    /// Get the root view out of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn root<C>(&self, cx: &mut C) -> Result<Entity<V>>
    where
        C: AppContext,
    {
        crate::Flatten::flatten(cx.update_window(self.any_handle, |root_view, _, _| {
            root_view
                .downcast::<V>()
                .map_err(|_| anyhow!("the type of the window's root view has changed"))
        }))
    }

    /// Updates the root view of this window.
    ///
    /// This will fail if the window has been closed or if the root view's type does not match
    pub fn update<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self.any_handle, |root_view, window, cx| {
            let view = root_view
                .downcast::<V>()
                .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

            Ok(view.update(cx, |view, cx| update(view, window, cx)))
        })?
    }

    /// Read the root view out of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn read<'a>(&self, cx: &'a App) -> Result<&'a V> {
        let x = cx
            .windows
            .get(self.id)
            .and_then(|window| {
                window
                    .as_deref()
                    .and_then(|window| window.root.clone())
                    .map(|root_view| root_view.downcast::<V>())
            })
            .context("window not found")?
            .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

        Ok(x.read(cx))
    }

    /// Read the root view out of this window, with a callback
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn read_with<C, R>(&self, cx: &C, read_with: impl FnOnce(&V, &App) -> R) -> Result<R>
    where
        C: AppContext,
    {
        cx.read_window(self, |root_view, cx| read_with(root_view.read(cx), cx))
    }

    /// Read the root view pointer off of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn entity<C>(&self, cx: &C) -> Result<Entity<V>>
    where
        C: AppContext,
    {
        cx.read_window(self, |root_view, _cx| root_view)
    }

    /// Check if this window is 'active'.
    ///
    /// Will return `None` if the window is closed or currently
    /// borrowed.
    pub fn is_active(&self, cx: &mut App) -> Option<bool> {
        cx.update_window(self.any_handle, |_, window, _| window.is_window_active())
            .ok()
    }
}

impl<V> Copy for WindowHandle<V> {}

impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> PartialEq for WindowHandle<V> {
    fn eq(&self, other: &Self) -> bool {
        self.any_handle == other.any_handle
    }
}

impl<V> Eq for WindowHandle<V> {}

impl<V> Hash for WindowHandle<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_handle.hash(state);
    }
}

impl<V: 'static> From<WindowHandle<V>> for AnyWindowHandle {
    fn from(val: WindowHandle<V>) -> Self {
        val.any_handle
    }
}

/// A handle to a window with any root view type, which can be downcast to a window with a specific root view type.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AnyWindowHandle {
    pub(crate) id: WindowId,
    state_type: TypeId,
}

impl AnyWindowHandle {
    /// Get the ID of this window.
    pub fn window_id(&self) -> WindowId {
        self.id
    }

    /// Attempt to convert this handle to a window handle with a specific root view type.
    /// If the types do not match, this will return `None`.
    pub fn downcast<T: 'static>(&self) -> Option<WindowHandle<T>> {
        if TypeId::of::<T>() == self.state_type {
            Some(WindowHandle {
                any_handle: *self,
                state_type: PhantomData,
            })
        } else {
            None
        }
    }

    /// Updates the state of the root view of this window.
    ///
    /// This will fail if the window has been closed.
    pub fn update<C, R>(
        self,
        cx: &mut C,
        update: impl FnOnce(AnyView, &mut Window, &mut App) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self, update)
    }

    /// Read the state of the root view of this window.
    ///
    /// This will fail if the window has been closed.
    pub fn read<T, C, R>(self, cx: &C, read: impl FnOnce(Entity<T>, &App) -> R) -> Result<R>
    where
        C: AppContext,
        T: 'static,
    {
        let view = self
            .downcast::<T>()
            .context("the type of the window's root view has changed")?;

        cx.read_window(&view, read)
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        self.platform_window.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, HandleError> {
        self.platform_window.display_handle()
    }
}

/// An identifier for an [`Element`].
///
/// Can be constructed with a string, a number, or both, as well
/// as other internal representations.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ElementId {
    /// The ID of a View element
    View(EntityId),
    /// An integer ID.
    Integer(u64),
    /// A string based ID.
    Name(SharedString),
    /// A UUID.
    Uuid(Uuid),
    /// An ID that's equated with a focus handle.
    FocusHandle(FocusId),
    /// A combination of a name and an integer.
    NamedInteger(SharedString, u64),
    /// A path.
    Path(Arc<std::path::Path>),
    /// A code location.
    CodeLocation(core::panic::Location<'static>),
    /// A labeled child of an element.
    NamedChild(Box<ElementId>, SharedString),
}

impl ElementId {
    /// Constructs an `ElementId::NamedInteger` from a name and `usize`.
    pub fn named_usize(name: impl Into<SharedString>, integer: usize) -> ElementId {
        Self::NamedInteger(name.into(), integer as u64)
    }
}

impl Display for ElementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementId::View(entity_id) => write!(f, "view-{}", entity_id)?,
            ElementId::Integer(ix) => write!(f, "{}", ix)?,
            ElementId::Name(name) => write!(f, "{}", name)?,
            ElementId::FocusHandle(_) => write!(f, "FocusHandle")?,
            ElementId::NamedInteger(s, i) => write!(f, "{}-{}", s, i)?,
            ElementId::Uuid(uuid) => write!(f, "{}", uuid)?,
            ElementId::Path(path) => write!(f, "{}", path.display())?,
            ElementId::CodeLocation(location) => write!(f, "{}", location)?,
            ElementId::NamedChild(id, name) => write!(f, "{}-{}", id, name)?,
        }

        Ok(())
    }
}

impl TryInto<SharedString> for ElementId {
    type Error = anyhow::Error;

    fn try_into(self) -> anyhow::Result<SharedString> {
        if let ElementId::Name(name) = self {
            Ok(name)
        } else {
            anyhow::bail!("element id is not string")
        }
    }
}

impl From<usize> for ElementId {
    fn from(id: usize) -> Self {
        ElementId::Integer(id as u64)
    }
}

impl From<i32> for ElementId {
    fn from(id: i32) -> Self {
        Self::Integer(id as u64)
    }
}

impl From<SharedString> for ElementId {
    fn from(name: SharedString) -> Self {
        ElementId::Name(name)
    }
}

impl From<Arc<std::path::Path>> for ElementId {
    fn from(path: Arc<std::path::Path>) -> Self {
        ElementId::Path(path)
    }
}

impl From<&'static str> for ElementId {
    fn from(name: &'static str) -> Self {
        ElementId::Name(name.into())
    }
}

impl<'a> From<&'a FocusHandle> for ElementId {
    fn from(handle: &'a FocusHandle) -> Self {
        ElementId::FocusHandle(handle.id)
    }
}

impl From<(&'static str, EntityId)> for ElementId {
    fn from((name, id): (&'static str, EntityId)) -> Self {
        ElementId::NamedInteger(name.into(), id.as_u64())
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, id): (&'static str, usize)) -> Self {
        ElementId::NamedInteger(name.into(), id as u64)
    }
}

impl From<(SharedString, usize)> for ElementId {
    fn from((name, id): (SharedString, usize)) -> Self {
        ElementId::NamedInteger(name, id as u64)
    }
}

impl From<(&'static str, u64)> for ElementId {
    fn from((name, id): (&'static str, u64)) -> Self {
        ElementId::NamedInteger(name.into(), id)
    }
}

impl From<Uuid> for ElementId {
    fn from(value: Uuid) -> Self {
        Self::Uuid(value)
    }
}

impl From<(&'static str, u32)> for ElementId {
    fn from((name, id): (&'static str, u32)) -> Self {
        ElementId::NamedInteger(name.into(), id.into())
    }
}

impl<T: Into<SharedString>> From<(ElementId, T)> for ElementId {
    fn from((id, name): (ElementId, T)) -> Self {
        ElementId::NamedChild(Box::new(id), name.into())
    }
}

impl From<&'static core::panic::Location<'static>> for ElementId {
    fn from(location: &'static core::panic::Location<'static>) -> Self {
        ElementId::CodeLocation(*location)
    }
}

/// A rectangle to be rendered in the window at the given position and size.
/// Passed as an argument [`Window::paint_quad`].
#[derive(Clone)]
pub struct PaintQuad {
    /// The bounds of the quad within the window.
    pub bounds: Bounds<Pixels>,
    /// The radii of the quad's corners.
    pub corner_radii: Corners<Pixels>,
    /// The background color of the quad.
    pub background: Background,
    /// The widths of the quad's borders.
    pub border_widths: Edges<Pixels>,
    /// The color of the quad's borders.
    pub border_color: Hsla,
    /// The style of the quad's borders.
    pub border_style: BorderStyle,
}

impl PaintQuad {
    /// Sets the corner radii of the quad.
    pub fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        PaintQuad {
            corner_radii: corner_radii.into(),
            ..self
        }
    }

    /// Sets the border widths of the quad.
    pub fn border_widths(self, border_widths: impl Into<Edges<Pixels>>) -> Self {
        PaintQuad {
            border_widths: border_widths.into(),
            ..self
        }
    }

    /// Sets the border color of the quad.
    pub fn border_color(self, border_color: impl Into<Hsla>) -> Self {
        PaintQuad {
            border_color: border_color.into(),
            ..self
        }
    }

    /// Sets the background color of the quad.
    pub fn background(self, background: impl Into<Background>) -> Self {
        PaintQuad {
            background: background.into(),
            ..self
        }
    }
}

/// Creates a quad with the given parameters.
pub fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Corners<Pixels>>,
    background: impl Into<Background>,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds,
        corner_radii: corner_radii.into(),
        background: background.into(),
        border_widths: border_widths.into(),
        border_color: border_color.into(),
        border_style,
    }
}

/// Creates a filled quad with the given bounds and background color.
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: background.into(),
        border_widths: (0.).into(),
        border_color: transparent_black(),
        border_style: BorderStyle::default(),
    }
}

/// Creates a rectangle outline with the given bounds, border color, and a 1px border width
pub fn outline(
    bounds: impl Into<Bounds<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: transparent_black().into(),
        border_widths: (1.).into(),
        border_color: border_color.into(),
        border_style,
    }
}

/// Olo: what was painted under a key ([`Window::paint_memo`]), and what
/// it was painted at.
#[derive(Clone)]
pub(crate) struct PaintMemo {
    bounds: Bounds<Pixels>,
    mask: ContentMask<Pixels>,
    opacity: f32,
    /// The scale it was painted at.
    scale: f32,
    range: Range<PaintIndex>,
    /// Which ways it can be moved (none with listeners).
    movable: Option<crate::scene::Movable>,
}

/// Olo: paint memos replay (`OLO_PAINT_MEMO=0` paints everything every
/// frame, to compare).
fn paint_memos() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("OLO_PAINT_MEMO").map_or(true, |v| v != "0"))
}

/// Olo: what a frame did that a frame's phases don't name (`OLO_FRAME_PHASES`):
/// lines shaped and glyphs drawn into the atlas, how many and how long.
pub(crate) struct FrameWork {
    pub shaped: [std::sync::atomic::AtomicU64; 2],
    pub rasterized: [std::sync::atomic::AtomicU64; 2],
    /// Lines' sprites found afresh (`paint_glyph_run`).
    pub runs: [std::sync::atomic::AtomicU64; 2],
}

pub(crate) static FRAME_WORK: FrameWork = FrameWork {
    shaped: [
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ],
    rasterized: [
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ],
    runs: [
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ],
};

impl FrameWork {
    pub(crate) fn note(&self, what: &[std::sync::atomic::AtomicU64; 2], since: std::time::Instant) {
        use std::sync::atomic::Ordering::Relaxed;
        what[0].fetch_add(1, Relaxed);
        what[1].fetch_add(since.elapsed().as_nanos() as u64, Relaxed);
    }

    /// (how many, ms) of each, since last taken.
    fn take(&self) -> [(u64, f64); 3] {
        use std::sync::atomic::Ordering::Relaxed;
        let t = |w: &[std::sync::atomic::AtomicU64; 2]| {
            (w[0].swap(0, Relaxed), w[1].swap(0, Relaxed) as f64 / 1e6)
        };
        [t(&self.shaped), t(&self.rasterized), t(&self.runs)]
    }
}

/// Olo: each frame's time in its phases and what its scene holds, on
/// stderr (`OLO_FRAME_PHASES=1`).
pub(crate) fn frame_phases() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("OLO_FRAME_PHASES").is_some())
}

/// Olo: a run of glyphs as [`Window::paint_glyph_run`] last found them in
/// the atlas (keep one per run, from frame to frame).
#[derive(Clone, Default)]
pub struct GlyphRun {
    key: Option<GlyphRunKey>,
    /// Each glyph's sprite, from the run's whole device pixel, its tile
    /// and colour.
    sprites: Vec<(Bounds<ScaledPixels>, AtlasTile, Hsla)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct GlyphRunKey {
    scale: u32,
    rendering: TextRendering,
    stamp: u64,
    phase: (i32, i32),
}

#[cfg(test)]
mod memo_tests {
    use crate::{
        AnyView, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Render,
        StyleRefinement, Styled as _, TestAppContext, Window, canvas, div, fill, point, px, rgb,
        size,
    };
    use std::{cell::Cell, rc::Rc};

    /// Three rows, each through a paint memo; how many times one was
    /// painted.
    struct Rows {
        painted: Rc<Cell<usize>>,
    }

    impl Render for Rows {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let painted = self.painted.clone();
            canvas(
                |_, _, _| {},
                move |b, _, window, cx| {
                    for i in 0..3u64 {
                        let row = Bounds {
                            origin: point(b.left(), b.top() + px(10.0 * i as f32)),
                            size: size(px(50.0), px(10.0)),
                        };
                        let painted = painted.clone();
                        window.paint_memo(i, row, cx, |window, _| {
                            painted.set(painted.get() + 1);
                            window.paint_quad(fill(row, rgb(0x336699)));
                        });
                    }
                },
            )
            .size_full()
        }
    }

    /// Something that changes, over the rows in a cached view.
    struct Outer {
        rows: Entity<Rows>,
        width: f32,
    }

    impl Render for Outer {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .child(div().w(px(self.width)).h(px(5.0)).bg(rgb(0x993333)))
                .child(div().size_full().child(
                    AnyView::from(self.rows.clone()).cached(StyleRefinement::default().size_full()),
                ))
        }
    }

    /// A cached view replayed whole keeps the memos painted in it: drawn
    /// again later, its rows are replayed, not painted again (they were,
    /// every row, the frame after a frame that replayed the view).
    #[crate::test]
    fn memos_survive_a_cached_view_replayed(cx: &mut TestAppContext) {
        let painted = Rc::new(Cell::new(0));
        let rows = cx.new(|_| Rows {
            painted: painted.clone(),
        });
        let (outer, cx) = cx.add_window_view(|_, _| Outer {
            rows: rows.clone(),
            width: 10.0,
        });
        assert_eq!(painted.get(), 3);
        // Only what's around it drawn again: the view replayed whole.
        outer.update(cx, |o, cx| {
            o.width = 20.0;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(painted.get(), 3);
        // The view drawn again: its rows replayed from the memos it kept.
        rows.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        assert_eq!(painted.get(), 3);
        let quads = cx.update(|window, _| window.rendered_frame.scene.quads.len());
        assert_eq!(quads, 4, "the three rows and the bar over them");
    }

    /// How many times it was drawn.
    struct Counted {
        renders: Rc<Cell<usize>>,
    }

    impl Render for Counted {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().size_full()
        }
    }

    /// An animation frame asked for outside drawing (in an event handler,
    /// as Tracker's band selection did) draws the window again, rather
    /// than panicking for want of a view being drawn.
    #[crate::test]
    fn an_animation_frame_asked_outside_drawing(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let (_view, cx) = cx.add_window_view(|_, _| Counted {
            renders: renders.clone(),
        });
        let before = renders.get();
        cx.update(|window, _| window.request_animation_frame());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(renders.get() > before, "drawn again");
    }
}
