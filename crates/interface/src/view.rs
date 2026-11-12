//! Views: what a window is made of, and how they're told things.
//!
//! A view is BeOS's `BView` and `BHandler` in one: it owns some state,
//! says what it shows for that state (`body`), and takes messages
//! (`message`), which are its own type, checked by the compiler. It
//! lives in one window, on that window's looper, and is never touched
//! from anywhere else: other threads send it messages through a
//! [`Messenger`].
//!
//! Within a window, views form a tree by ownership: a view holds the
//! views it made as [`Child`]ren (a handle that can't be copied), reaches
//! down into them with `update`, and hears from them by the events they
//! `emit`. Nothing reaches up or sideways except by a message, taken
//! after what's running now has finished. So a view is never read or
//! changed while it's in the middle of taking something else: there is
//! no way to write it.

use std::{
    any::{Any, TypeId},
    cell::RefCell,
    marker::PhantomData,
    ops::{Deref, DerefMut},
    rc::Rc,
    time::Duration,
};

use olo_app::{Messenger, Task};

use crate::{
    Element, WindowId,
    element::Kind,
    tree::ViewId,
    window::{WindowCx, deliver_to_view},
};

/// A part of a window with state of its own.
pub trait View: Sized + 'static {
    /// What it can be told: an enum of everything that happens to it (a
    /// button of its own pressed, a worker's result, a child's news).
    /// `()` for a view that's told nothing.
    type Message: 'static;

    /// What it shows, for the state it has now. Called when the view is
    /// first shown and after each [`Cx::invalidate`]; what comes of it is
    /// compared with what the window has, and only what differs is laid
    /// out or drawn again.
    fn body(&mut self, cx: &mut Cx<Self>) -> Element;

    /// Takes a message: BeOS's `MessageReceived`.
    fn message(&mut self, _message: Self::Message, _cx: &mut Cx<Self>) {}

    /// Takes a command by name: what a menu item, a tool bar's button,
    /// the command palette or an agent chose. Whether it was this view's
    /// to take.
    fn command(&mut self, _command: &str, _cx: &mut Cx<Self>) -> bool {
        false
    }

    /// The person asked to close the window (for the window's own view):
    /// whether it may go now. BeOS's `QuitRequested`.
    fn close_requested(&mut self, _cx: &mut Cx<Self>) -> bool {
        true
    }

    /// The window got the keyboard, or lost it.
    fn window_activated(&mut self, _active: bool, _cx: &mut Cx<Self>) {}

    /// The window is closing (for the window's own view): the last
    /// thing it's told.
    fn window_closing(&mut self, _cx: &mut Cx<Self>) {}

    /// Whether the window takes what's dragged over it part by part
    /// (for the window's own view): it is then asked where
    /// ([`View::drop_over`]) and handed what's dropped
    /// ([`View::dropped`]). A window that doesn't takes files alone,
    /// copied, wherever an element takes them
    /// (`InteractiveElement::on_drop`).
    fn takes_drops(&self) -> bool {
        false
    }

    /// Something is dragged over the window, at `at` (asked as it comes,
    /// as it moves and as a modifier key changes; `offer` None: it left,
    /// or was dropped): the part that would take it there, as which of
    /// its types and how, or None to refuse it. The part's name comes
    /// back with the drop. Nothing is drawn for it: a view that shows
    /// where it would go keeps that and invalidates itself when it
    /// changes.
    fn drop_over(&mut self, _offer: Option<&crate::DropOffer>, _at: crate::Point<crate::Pixels>, _cx: &mut Cx<Self>) -> Option<crate::DropAccept> {
        None
    }

    /// What was dropped where [`View::drop_over`] took it, its data read.
    fn dropped(&mut self, _dropped: crate::Dropped, _cx: &mut Cx<Self>) {}

    /// A display came, went or changed (for the window's first view and
    /// its own): `displays()` lists them now.
    fn displays_changed(&mut self, _cx: &mut Cx<Self>) {}

    /// What to keep of it across an upgrade in place or the display
    /// thread starting again (for the window's own view): handed back to
    /// the program as it starts again.
    fn save_state(&mut self, _cx: &mut Cx<Self>) -> Option<String> {
        None
    }
}

/// A view that takes typing: BeOS's `BTextView`, as far as the keyboard
/// goes. Its element says so with `Cx::text_input`.
pub trait TextInput: View {
    /// Puts `text` in, in place of the selection (or of what an input
    /// method was composing).
    fn insert_text(&mut self, text: &str, cx: &mut Cx<Self>);

    /// Shows `text` as what an input method is composing so far.
    fn set_marked_text(&mut self, text: &str, cx: &mut Cx<Self>);

    /// What was being composed stays as it is.
    fn unmark_text(&mut self, cx: &mut Cx<Self>);

    /// What was being composed goes.
    fn delete_marked_text(&mut self, cx: &mut Cx<Self>);
}

/// A view of any type, as the window keeps it.
pub(crate) trait AnyView: 'static {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn body(&mut self, id: ViewId, cx: &mut WindowCx) -> Element;
    fn command(&mut self, id: ViewId, command: &str, cx: &mut WindowCx) -> bool;
    fn close_requested(&mut self, id: ViewId, cx: &mut WindowCx) -> bool;
    fn window_activated(&mut self, id: ViewId, active: bool, cx: &mut WindowCx);
    fn window_closing(&mut self, id: ViewId, cx: &mut WindowCx);
    fn displays_changed(&mut self, id: ViewId, cx: &mut WindowCx);
    fn save_state(&mut self, id: ViewId, cx: &mut WindowCx) -> Option<String>;
    fn takes_drops(&self) -> bool;
    fn drop_over(&mut self, id: ViewId, offer: Option<&crate::DropOffer>, at: crate::Point<crate::Pixels>, cx: &mut WindowCx) -> Option<crate::DropAccept>;
    fn dropped(&mut self, id: ViewId, dropped: crate::Dropped, cx: &mut WindowCx);
    fn type_name(&self) -> &'static str;
}

impl<V: View> AnyView for V {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn body(&mut self, id: ViewId, cx: &mut WindowCx) -> Element {
        View::body(self, &mut Cx::new(cx, id))
    }

    fn command(&mut self, id: ViewId, command: &str, cx: &mut WindowCx) -> bool {
        View::command(self, command, &mut Cx::new(cx, id))
    }

    fn close_requested(&mut self, id: ViewId, cx: &mut WindowCx) -> bool {
        View::close_requested(self, &mut Cx::new(cx, id))
    }

    fn window_activated(&mut self, id: ViewId, active: bool, cx: &mut WindowCx) {
        View::window_activated(self, active, &mut Cx::new(cx, id))
    }

    fn takes_drops(&self) -> bool {
        View::takes_drops(self)
    }

    fn drop_over(&mut self, id: ViewId, offer: Option<&crate::DropOffer>, at: crate::Point<crate::Pixels>, cx: &mut WindowCx) -> Option<crate::DropAccept> {
        View::drop_over(self, offer, at, &mut Cx::new(cx, id))
    }

    fn dropped(&mut self, id: ViewId, dropped: crate::Dropped, cx: &mut WindowCx) {
        View::dropped(self, dropped, &mut Cx::new(cx, id))
    }

    fn save_state(&mut self, id: ViewId, cx: &mut WindowCx) -> Option<String> {
        View::save_state(self, &mut Cx::new(cx, id))
    }

    fn displays_changed(&mut self, id: ViewId, cx: &mut WindowCx) {
        View::displays_changed(self, &mut Cx::new(cx, id))
    }

    fn window_closing(&mut self, id: ViewId, cx: &mut WindowCx) {
        View::window_closing(self, &mut Cx::new(cx, id))
    }

    fn type_name(&self) -> &'static str {
        std::any::type_name::<V>()
    }
}

/// Takes what a view emits, for whoever subscribed.
pub(crate) type Subscriber = Rc<dyn Fn(&dyn Any, &mut WindowCx)>;

pub(crate) struct ViewSlot {
    /// None while the view is taking something (it's out).
    pub(crate) view: Option<Box<dyn AnyView>>,
    pub(crate) root: Option<crate::tree::NodeId>,
    /// Its `body` is to run again.
    pub(crate) dirty: bool,
    /// Its `body` asked the window's size: it runs again when that
    /// changes.
    pub(crate) reads_size: bool,
    /// Its `body` asked whether the window has the keyboard: it runs
    /// again when that changes.
    pub(crate) reads_active: bool,
    /// Who hears what it emits, by the event's type.
    pub(crate) subscribers: Vec<(TypeId, Subscriber)>,
    /// Timers and tasks that end with it.
    pub(crate) timers: Vec<olo_app::TimerId>,
}

thread_local! {
    /// Views whose owner let go of them, for their windows to end.
    static DROPPED: RefCell<Vec<(WindowId, ViewId)>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn take_dropped() -> Vec<(WindowId, ViewId)> {
    DROPPED.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

/// A view owned by another: made with [`Cx::new`], kept in the owner's
/// state, placed in its `body` (it's an element: `.child(&self.field)`
/// or [`Child::element`]). It can't be copied: one owner, who alone can
/// reach into it, so nothing can reach a view that's busy. When it's
/// dropped the view ends.
pub struct Child<V> {
    id: ViewId,
    window: WindowId,
    _view: PhantomData<fn() -> V>,
}

impl<V: View> Child<V> {
    pub(crate) fn new(id: ViewId, window: WindowId) -> Self {
        Child { id, window, _view: PhantomData }
    }

    /// Where it goes in its owner's `body`.
    pub fn element(&self) -> Element {
        Element::new(Kind::View(self.id))
    }

    /// Reaches into the view. (Its own handlers run later: what it
    /// emits is heard after this returns.)
    pub fn update<R>(&self, cx: &mut WindowCx, f: impl FnOnce(&mut V, &mut Cx<V>) -> R) -> R {
        cx.update_view(self.id, f).expect("a view is reached only by its owner, who can't be inside it")
    }

    /// Reads the view.
    pub fn read<'a>(&self, cx: &'a WindowCx) -> &'a V {
        cx.read_view(self.id).expect("a view is reached only by its owner, who can't be inside it")
    }

    /// Sends it a message, taken once what's running now is done.
    pub fn send(&self, cx: &mut WindowCx, message: V::Message) {
        self.downgrade().send(cx, message)
    }

    /// Gives it the keyboard (the first thing in it that takes it).
    pub fn focus(&self, cx: &mut WindowCx) {
        cx.focus_view(self.id);
    }

    /// Whether it, or something in it, has the keyboard.
    pub fn is_focused(&self, cx: &WindowCx) -> bool {
        cx.view_has_focus(self.id)
    }

    /// A handle that doesn't own it: to send it messages with.
    pub fn downgrade(&self) -> ViewRef<V> {
        ViewRef { id: self.id, window: self.window, _view: PhantomData }
    }

    pub(crate) fn id(&self) -> ViewId {
        self.id
    }
}

/// A [`Child`] of whatever type: owned the same way, placed the same
/// way, for an owner that needn't know what it holds (a window's frame
/// round the program's view).
pub struct AnyChild {
    id: ViewId,
    window: WindowId,
}

impl AnyChild {
    /// Where it goes in its owner's `body`.
    pub fn element(&self) -> Element {
        Element::new(Kind::View(self.id))
    }
}

impl<V: View> From<Child<V>> for AnyChild {
    fn from(child: Child<V>) -> AnyChild {
        let any = AnyChild { id: child.id, window: child.window };
        // (The view goes on, under its new handle.)
        std::mem::forget(child);
        any
    }
}

impl Drop for AnyChild {
    fn drop(&mut self) {
        let _ = DROPPED.try_with(|d| d.borrow_mut().push((self.window, self.id)));
    }
}

impl crate::IntoElement for &AnyChild {
    fn into_element(self) -> Element {
        self.element()
    }
}

impl<V> Drop for Child<V> {
    fn drop(&mut self) {
        let _ = DROPPED.try_with(|d| d.borrow_mut().push((self.window, self.id)));
    }
}

impl<V: View> crate::IntoElement for &Child<V> {
    fn into_element(self) -> Element {
        self.element()
    }
}

/// A view by name, not owned: something to send messages to. It can be
/// copied and kept anywhere on the window's looper (a messenger, made
/// from it, goes to other threads). Nothing can be read or changed
/// through it at once, only asked for: the message is taken when the
/// view is free.
pub struct ViewRef<V> {
    id: ViewId,
    window: WindowId,
    _view: PhantomData<fn() -> V>,
}

impl<V> Clone for ViewRef<V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<V> Copy for ViewRef<V> {}

impl<V> PartialEq for ViewRef<V> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.window == other.window
    }
}

impl<V: View> ViewRef<V> {
    /// Sends the view a message, taken once what's running now is done.
    pub fn send(&self, cx: &mut WindowCx, message: V::Message) {
        let (window, view) = (self.window, self.id);
        cx.looper.defer(move |looper| deliver_to_view::<V>(looper, window, view, |v, cx| v.message(message, cx)));
    }

    /// Has `f` run on the view once what's running now is done.
    pub fn update(&self, cx: &mut WindowCx, f: impl FnOnce(&mut V, &mut Cx<V>) + 'static) {
        let (window, view) = (self.window, self.id);
        cx.looper.defer(move |looper| deliver_to_view::<V>(looper, window, view, f));
    }

    /// Gives it the keyboard (the first thing in it that takes it).
    pub fn focus(&self, cx: &mut WindowCx) {
        cx.focus_view(self.id);
    }

    /// Whether it, or something in it, has the keyboard.
    pub fn is_focused(&self, cx: &WindowCx) -> bool {
        cx.view_has_focus(self.id)
    }

    /// A messenger to the view, for other threads (a worker's results).
    pub fn messenger(&self, cx: &WindowCx) -> Messenger<V::Message>
    where
        V::Message: Send,
    {
        let (window, view) = (self.window, self.id);
        Messenger::new(cx.looper.handle(), move |looper, message| deliver_to_view::<V>(looper, window, view, |v, cx| v.message(message, cx)))
    }

    pub(crate) fn id(&self) -> ViewId {
        self.id
    }
}

/// Has a view say what it shows again, from any thread
/// ([`Cx::invalidator`]). Cheap to clone; a view that has gone hears
/// nothing.
#[derive(Clone)]
pub struct Invalidator(Messenger<()>);

impl Invalidator {
    pub fn invalidate(&self) {
        self.0.send(());
    }
}

/// Has something done to a view from any thread, once what its looper
/// is doing is done ([`Cx::doer`]): for a part of a view that any app's
/// view may hold (an editor, with no message of its host's to send) to
/// hear from its workers. A view's own workers send it its messages
/// ([`Cx::messenger`]).
pub struct Doer<V>(Messenger<Deed<V>>);

type Deed<V> = Box<dyn for<'a, 'b, 'c> FnOnce(&'c mut V, &'c mut Cx<'a, 'b, V>) + Send>;

impl<V> Clone for Doer<V> {
    fn clone(&self) -> Self {
        Doer(self.0.clone())
    }
}

impl<V: View> Doer<V> {
    /// Has `f` run on the view (nothing, if it has gone). False if its
    /// looper has ended.
    pub fn run(&self, f: impl for<'a, 'b, 'c> FnOnce(&'c mut V, &'c mut Cx<'a, 'b, V>) + Send + 'static) -> bool {
        self.0.send(Box::new(f))
    }
}

/// What a view is handed with everything it's told: the window it's in
/// (it derefs to [`WindowCx`]) and itself.
pub struct Cx<'a, 'b, V> {
    window: &'a mut WindowCx<'b>,
    view: ViewId,
    _view: PhantomData<fn() -> V>,
}

impl<'b, V> Deref for Cx<'_, 'b, V> {
    type Target = WindowCx<'b>;

    fn deref(&self) -> &WindowCx<'b> {
        self.window
    }
}

impl<'b, V> DerefMut for Cx<'_, 'b, V> {
    fn deref_mut(&mut self) -> &mut WindowCx<'b> {
        self.window
    }
}

impl<'a, 'b, V: View> Cx<'a, 'b, V> {
    pub(crate) fn new(window: &'a mut WindowCx<'b>, view: ViewId) -> Self {
        Cx { window, view, _view: PhantomData }
    }

    /// What the view shows changed: its `body` runs again before the
    /// next frame. BeOS's `Invalidate`. (Once however often it's called
    /// before then.)
    pub fn invalidate(&mut self) {
        let view = self.view;
        self.window.core.invalidate_view(view);
    }

    /// (GPUI's name for [`Cx::invalidate`].)
    pub fn notify(&mut self) {
        self.invalidate()
    }

    /// What the view shows changed, and changes faster than frames are
    /// shown (a shell printing, a meter fed by a stream): its `body`
    /// runs again for the next frame drawn, once, at the last moment.
    /// At once when no frame waits to be shown, so one change alone is
    /// seen as soon as with [`Cx::invalidate`]; while one waits, when
    /// the compositor asks for the next, so however much changes
    /// meanwhile is laid out and painted once, as it is then. (A window
    /// that isn't shown is asked for no frame: its views run again when
    /// it is, or within a second.)
    pub fn invalidate_at_frame(&mut self) {
        let view = self.view;
        self.window.core.invalidate_view_at_frame(view);
    }

    /// Makes a view this one owns.
    pub fn new_view<T: View>(&mut self, build: impl FnOnce(&mut Cx<T>) -> T) -> Child<T> {
        self.window.new_view(build)
    }

    /// This view, to send messages to (for a closure, a child, a
    /// worker).
    pub fn view_ref(&self) -> ViewRef<V> {
        ViewRef { id: self.view, window: self.window.core.id, _view: PhantomData }
    }

    /// A messenger to this view, for other threads.
    pub fn messenger(&self) -> Messenger<V::Message>
    where
        V::Message: Send,
    {
        self.view_ref().messenger(self.window)
    }

    /// A way to have this view say what it shows again, from any thread:
    /// for what's made ready elsewhere with no message of the view's own
    /// to bring it (a dictionary read on a worker, for an editor in any
    /// app's view).
    pub fn invalidator(&self) -> Invalidator {
        let (window, view) = (self.window.core.id, self.view);
        Invalidator(Messenger::new(self.window.looper.handle(), move |looper, ()| crate::window::deliver_to_view::<V>(looper, window, view, |_, cx| cx.invalidate())))
    }

    /// A way to have something done to this view from any thread
    /// ([`Doer`]).
    pub fn doer(&self) -> Doer<V> {
        let (window, view) = (self.window.core.id, self.view);
        Doer(Messenger::new(self.window.looper.handle(), move |looper, deed: Deed<V>| crate::window::deliver_to_view::<V>(looper, window, view, |v, cx| deed(v, cx))))
    }

    /// Sends this view a message, taken once what's running now is done.
    pub fn send(&mut self, message: V::Message) {
        self.view_ref().send(self.window, message)
    }

    /// A handler that hands what happened to this view.
    pub fn listener<E: ?Sized + 'static>(&self, f: impl Fn(&mut V, &E, &mut Cx<V>) + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        let view = self.view;
        move |event, cx| {
            cx.update_view::<V, _>(view, |this, cx| f(this, event, cx));
        }
    }

    /// A handler that sends this view `message` whatever happened: a
    /// button's `on_click(cx.msg(Msg::Save))`.
    pub fn msg<E: ?Sized + 'static>(&self, message: V::Message) -> impl Fn(&E, &mut WindowCx) + 'static
    where
        V::Message: Clone,
    {
        let view = self.view;
        move |_, cx| {
            let message = message.clone();
            cx.update_view::<V, _>(view, |this, cx| this.message(message, cx));
        }
    }

    /// A handler that sends this view the message `f` makes of what
    /// happened.
    pub fn msg_with<E: ?Sized + 'static>(&self, f: impl Fn(&E) -> V::Message + 'static) -> impl Fn(&E, &mut WindowCx) + 'static {
        let view = self.view;
        move |event, cx| {
            let message = f(event);
            cx.update_view::<V, _>(view, |this, cx| this.message(message, cx));
        }
    }

    /// A handler for an element that takes typing for this view
    /// (`InteractiveElement::on_text_input`).
    pub fn text_input(&self) -> impl Fn(&crate::TextInputEvent, &mut WindowCx) + 'static
    where
        V: TextInput,
    {
        let view = self.view;
        move |event, cx| {
            cx.update_view::<V, _>(view, |this, cx| match event {
                crate::TextInputEvent::Insert(text) => this.insert_text(text, cx),
                crate::TextInputEvent::Mark(text) => this.set_marked_text(text, cx),
                crate::TextInputEvent::Unmark => this.unmark_text(cx),
                crate::TextInputEvent::DeleteMarked => this.delete_marked_text(cx),
            });
        }
    }

    /// Gives this view the keyboard: the first thing in it that takes
    /// it. (A view not shown yet takes it when it is.)
    pub fn focus(&mut self) {
        let view = self.view;
        self.window.focus_view(view);
    }

    /// Whether this view, or something in it, has the keyboard.
    pub fn is_focused(&self) -> bool {
        self.window.view_has_focus(self.view)
    }

    /// Where something this view drew is in the window now: `bounds`
    /// as the view's drawing had it, moved as far as what holds the view
    /// has scrolled.
    pub fn to_window(&self, bounds: crate::Bounds<crate::Pixels>) -> crate::Bounds<crate::Pixels> {
        let Some(root) = self.window.core.views.get(self.view).and_then(|v| v.root) else { return bounds };
        crate::Bounds { origin: bounds.origin + self.window.core.shift_at(root), size: bounds.size }
    }

    /// A drawing of this view's (an element for its `body`): `paint` is
    /// handed the view as it is when the drawing is wanted.
    pub fn canvas(&self, paint: impl Fn(&V, &mut crate::Painter, crate::Bounds<crate::Pixels>) + 'static) -> Element {
        crate::view_canvas(paint)
    }

    /// A handler that asks this view something and hands back its
    /// answer (None if the view is busy or gone): for what must know at
    /// once (whether a press on a row was taken).
    pub fn asks<E: ?Sized + 'static, R: 'static>(&self, f: impl Fn(&mut V, &E, &mut Cx<V>) -> R + 'static) -> impl Fn(&E, &mut WindowCx) -> Option<R> + 'static {
        let view = self.view;
        move |event, cx| cx.update_view::<V, _>(view, |this, cx| f(this, event, cx))
    }

    /// Tells whoever subscribed to this view something happened to it
    /// (a field's text changed, a list's row was chosen). They hear it
    /// once what's running now is done.
    pub fn emit<E: 'static>(&mut self, event: E) {
        let view = self.view;
        let subscribers: Vec<Subscriber> =
            self.window.core.views.get(view).map(|slot| slot.subscribers.iter().filter(|(t, _)| *t == TypeId::of::<E>()).map(|(_, s)| s.clone()).collect()).unwrap_or_default();
        if subscribers.is_empty() {
            return;
        }
        let window = self.window.core.id;
        self.window.looper.defer(move |looper| {
            crate::window::with_window(looper, window, |cx| {
                for subscriber in &subscribers {
                    subscriber(&event, cx);
                }
            });
        });
    }

    /// Hears what a view this one owns emits.
    pub fn subscribe<T: View, E: 'static>(&mut self, child: &Child<T>, f: impl Fn(&mut V, &E, &mut Cx<V>) + 'static) {
        let view = self.view;
        let subscriber: Subscriber = Rc::new(move |event, cx| {
            if let Some(event) = event.downcast_ref::<E>() {
                cx.update_view::<V, _>(view, |this, cx| f(this, event, cx));
            }
        });
        if let Some(slot) = self.window.core.views.get_mut(child.id()) {
            slot.subscribers.push((TypeId::of::<E>(), subscriber));
        }
    }

    /// Sends this view `message` after `delay` (unless the view has
    /// ended): BeOS's `BMessageRunner`, once.
    pub fn after(&mut self, delay: Duration, message: V::Message) {
        let (window, view) = (self.window.core.id, self.view);
        self.window.looper.after(delay, move |looper| deliver_to_view::<V>(looper, window, view, |v, cx| v.message(message, cx)));
    }

    /// Has `f` run on this view after `delay`, unless the view has
    /// ended or the timer is cancelled (`cx.cancel(timer)`): for what a
    /// view has due at a time it works out as it goes (the kit's `Due`),
    /// where a message of its own would say no more.
    pub fn after_do(&mut self, delay: Duration, f: impl FnOnce(&mut V, &mut Cx<V>) + 'static) -> olo_app::TimerId {
        let (window, view) = (self.window.core.id, self.view);
        self.window.looper.after(delay, move |looper| deliver_to_view::<V>(looper, window, view, f))
    }

    /// Sends this view a message every `interval` until the view ends
    /// or the timer is cancelled (`cx.cancel(timer)`). Not for looking
    /// for changes: a clock's tick, a caret's blink.
    pub fn every(&mut self, interval: Duration, message: impl Fn() -> V::Message + 'static) -> olo_app::TimerId {
        let (window, view) = (self.window.core.id, self.view);
        let timer = self.window.looper.every(interval, move |looper| deliver_to_view::<V>(looper, window, view, |v, cx| v.message(message(), cx)));
        if let Some(slot) = self.window.core.views.get_mut(view) {
            slot.timers.push(timer);
        }
        timer
    }

    /// Runs a future on the window's looper for this view: for waiting
    /// on work done elsewhere (`olo_app::pool`) without blocking. It's
    /// handed a [`ViewRef`] to send what comes of it.
    pub fn spawn<F, T>(&mut self, f: impl FnOnce(ViewRef<V>) -> F) -> Task<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let future = f(self.view_ref());
        self.window.looper.spawn_local(future)
    }

    /// Does `work` on a worker thread and sends this view what `then`
    /// makes of its result: the way to do anything that might take
    /// longer than a frame.
    pub fn work<T: Send + 'static>(&mut self, work: impl FnOnce() -> T + Send + 'static, then: impl FnOnce(T) -> V::Message + Send + 'static)
    where
        V::Message: Send,
    {
        let messenger = self.messenger();
        olo_app::pool::run(move || {
            messenger.send(then(work()));
        })
        .detach();
    }

    pub(crate) fn id(&self) -> ViewId {
        self.view
    }
}
