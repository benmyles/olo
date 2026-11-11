//! Elements: what a view's `body` says it shows.
//!
//! An element is a description, made afresh each time a view's `body`
//! runs and thrown away once the window's tree has been brought in line
//! with it (`crate::tree`): what's the same as last time keeps its
//! place, its layout and what it drew. So `body` can be written as what
//! the view shows for the state it has, with no code to keep a tree in
//! step by hand, and still nothing is laid out or drawn again but what
//! differs.

use std::{any::Any, fmt::Display, rc::Rc, sync::Arc};

use smallvec::SmallVec;

use crate::{
    Bounds, ClickEvent, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, Pixels, Point, ScrollWheelEvent, SharedString, Size,
    StyleRefinement, Styled, TextRun, WindowCx, tree::ViewId,
};

/// What tells one child from another when a view's `body` runs again
/// (a list's rows, by what each shows): without one, children are told
/// apart by their order.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ElementId {
    Integer(u64),
    Name(SharedString),
    NamedInteger(SharedString, u64),
    Path(Arc<std::path::Path>),
    NamedChild(Box<ElementId>, SharedString),
}

impl ElementId {
    pub fn named_usize(name: impl Into<SharedString>, integer: usize) -> ElementId {
        Self::NamedInteger(name.into(), integer as u64)
    }
}

impl Display for ElementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementId::Integer(ix) => write!(f, "{ix}"),
            ElementId::Name(name) => write!(f, "{name}"),
            ElementId::NamedInteger(s, i) => write!(f, "{s}-{i}"),
            ElementId::Path(path) => write!(f, "{}", path.display()),
            ElementId::NamedChild(id, name) => write!(f, "{id}-{name}"),
        }
    }
}

impl From<usize> for ElementId {
    fn from(id: usize) -> Self {
        ElementId::Integer(id as u64)
    }
}

impl From<u64> for ElementId {
    fn from(id: u64) -> Self {
        ElementId::Integer(id)
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

impl From<String> for ElementId {
    fn from(name: String) -> Self {
        ElementId::Name(name.into())
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

impl From<(ElementId, &'static str)> for ElementId {
    fn from((id, name): (ElementId, &'static str)) -> Self {
        ElementId::NamedChild(Box::new(id), name.into())
    }
}

/// What a control draws itself as: the pointer over it, held down on
/// it, with the keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlState {
    /// The pointer is over it.
    pub hovered: bool,
    /// Held down on it, the pointer still over it.
    pub pressed: bool,
    pub enabled: bool,
    /// It has the keyboard.
    pub focused: bool,
}

/// Draws something: handed the view whose `body` made it (as it is when
/// the drawing is wanted), what to draw with, where, and how the pointer
/// and keyboard stand to it.
pub(crate) type PaintFn = Rc<dyn Fn(&dyn Any, &mut Painter, Bounds<Pixels>, ControlState)>;
/// Sizes something by what it holds, given what's known of its size and
/// the room it has.
pub(crate) type MeasureFn = Rc<dyn Fn(&dyn Any, Size<Option<Pixels>>, Size<crate::AvailableSpace>, &mut crate::tree::Measuring) -> Size<Pixels>>;
/// Takes something that happened (on the window's looper).
pub(crate) type Listener<E> = Rc<dyn Fn(&E, &mut WindowCx)>;

/// What typing does to the text being edited: what a key types, and
/// what an input method composes.
#[derive(Clone, Debug)]
pub enum TextInputEvent {
    /// Text to put in, in place of the selection (or of what's being
    /// composed).
    Insert(String),
    /// The text being composed so far, shown marked until it's done.
    Mark(String),
    /// What was being composed stays as it is.
    Unmark,
    /// What was being composed goes.
    DeleteMarked,
}

pub(crate) struct CanvasSpec {
    pub(crate) paint: PaintFn,
    /// What the drawing depends on, if the element says: the same key as
    /// last time (and the same size and state), it isn't drawn again when
    /// its view's `body` runs.
    pub(crate) key: Option<u64>,
    /// The key, worked out from the view when the element takes its
    /// place in the tree (`key_by`).
    pub(crate) key_by: Option<KeyFn>,
    pub(crate) measure: Option<MeasureFn>,
}

pub(crate) type KeyFn = Rc<dyn Fn(&dyn Any) -> u64>;

pub(crate) struct TextSpec {
    pub(crate) text: SharedString,
    /// Its runs, if it has more than one look.
    pub(crate) runs: Option<Vec<TextRun>>,
}

pub(crate) enum Kind {
    /// Holds other elements, laid out by its style.
    Group,
    Text(TextSpec),
    Canvas(CanvasSpec),
    /// Another view's place.
    View(ViewId),
}

impl Kind {
    pub(crate) fn tag(&self) -> u8 {
        match self {
            Kind::Group => 0,
            Kind::Text(_) => 1,
            Kind::Canvas(_) => 2,
            Kind::View(_) => 3,
        }
    }
}

/// What agents read a part of a window as.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AgentSpec {
    pub(crate) role: &'static str,
    pub(crate) name: Option<SharedString>,
    pub(crate) value: Option<SharedString>,
    pub(crate) checked: Option<bool>,
    pub(crate) selected: Option<bool>,
    /// Whether it takes clicks, if that isn't to be read off its
    /// handlers.
    pub(crate) clickable: Option<bool>,
}

/// How an element takes the pointer and the keyboard.
#[derive(Default)]
pub(crate) struct Interactivity {
    pub(crate) mouse_down: SmallVec<[(Option<MouseButton>, Listener<MouseDownEvent>); 1]>,
    pub(crate) mouse_up: SmallVec<[(Option<MouseButton>, Listener<MouseUpEvent>); 1]>,
    /// A release anywhere but over it.
    pub(crate) mouse_up_out: SmallVec<[(Option<MouseButton>, Listener<MouseUpEvent>); 1]>,
    pub(crate) mouse_move: SmallVec<[Listener<MouseMoveEvent>; 1]>,
    pub(crate) scroll_wheel: SmallVec<[Listener<ScrollWheelEvent>; 1]>,
    /// Files dropped on it, dragged from somewhere else.
    pub(crate) file_drop: SmallVec<[Listener<crate::ExternalPaths>; 1]>,
    pub(crate) click: SmallVec<[Listener<ClickEvent>; 1]>,
    pub(crate) hover: SmallVec<[Listener<bool>; 1]>,
    pub(crate) key_down: SmallVec<[Listener<KeyDownEvent>; 1]>,
    pub(crate) key_up: SmallVec<[Listener<KeyUpEvent>; 1]>,
    /// Keys on their way down to what has the keyboard, before it.
    pub(crate) capture_key_down: SmallVec<[Listener<KeyDownEvent>; 1]>,
    pub(crate) modifiers_changed: SmallVec<[Listener<ModifiersChangedEvent>; 1]>,
    /// The pointer anywhere in the window, over this or not (a drag that
    /// began here follows it out).
    pub(crate) window_mouse_down: SmallVec<[Listener<MouseDownEvent>; 1]>,
    pub(crate) window_mouse_up: SmallVec<[Listener<MouseUpEvent>; 1]>,
    pub(crate) window_mouse_move: SmallVec<[Listener<MouseMoveEvent>; 1]>,
    /// Keys nothing with the keyboard took (the window's own: its tab
    /// keys), wherever the keyboard is in the window.
    pub(crate) window_key_down: SmallVec<[Listener<KeyDownEvent>; 1]>,
    pub(crate) focus_changed: SmallVec<[Listener<bool>; 1]>,
    /// It takes typing while it has the keyboard.
    pub(crate) text_input: Option<Listener<TextInputEvent>>,
    pub(crate) hover_style: Option<Box<StyleRefinement>>,
    pub(crate) active_style: Option<Box<StyleRefinement>>,
    pub(crate) focus_style: Option<Box<StyleRefinement>>,
    /// What's under it gets nothing of the pointer.
    pub(crate) occlude: bool,
    /// Drawn over everything else in the window, and first under the
    /// pointer ([`InteractiveElement::raised`]).
    pub(crate) raised: bool,
    /// It can have the keyboard.
    pub(crate) focusable: bool,
    /// A click on it gives it the keyboard.
    pub(crate) click_focus: bool,
    /// Tab reaches it.
    pub(crate) tab_stop: bool,
    /// It counts as having the keyboard while anything it holds has it.
    pub(crate) focus_within: bool,
    /// Its scroller, if a view keeps one for it (`track_scroll`).
    pub(crate) scroll: Option<crate::ScrollHandle>,
    /// It hears where it's scrolled to, when that changes.
    pub(crate) scroll_listeners: SmallVec<[Listener<Point<Pixels>>; 1]>,
    /// It hears where layout put it, when that changes.
    pub(crate) bounds_listeners: SmallVec<[Listener<Bounds<Pixels>>; 1]>,
    /// Scrollers it draws by (a scroll bar): it's drawn again when one
    /// moves or its ends do.
    pub(crate) watches: SmallVec<[(u64, u8); 1]>,
    /// The name the keyboard is given to it by (`track_focus`).
    pub(crate) focus_handle: Option<u64>,
    /// Its text isn't told to agents as a label of its own (what holds
    /// it says what it is: a document's text).
    pub(crate) unlabelled: bool,
    /// It's drawn anew when the pointer comes over it, presses it or
    /// leaves, or the keyboard comes or goes.
    pub(crate) paints_by_state: bool,
    pub(crate) disabled: bool,
    /// Return clicks it from anywhere in its window, when nothing that
    /// has the keyboard takes Return (a default button).
    pub(crate) default_key: bool,
    pub(crate) agent: Option<AgentSpec>,
}

impl Interactivity {
    /// Whether the pointer being over it matters to anything.
    pub(crate) fn takes_pointer(&self) -> bool {
        self.occlude
            || self.paints_by_state
            || self.hover_style.is_some()
            || self.active_style.is_some()
            || !(self.mouse_down.is_empty()
                && self.mouse_up.is_empty()
                && self.mouse_move.is_empty()
                && self.scroll_wheel.is_empty()
                && self.click.is_empty()
                && self.hover.is_empty()
                && self.file_drop.is_empty())
    }

    pub(crate) fn hears_window(&self) -> bool {
        self.default_key
            || !(self.window_mouse_down.is_empty() && self.window_mouse_up.is_empty() && self.window_mouse_move.is_empty() && self.mouse_up_out.is_empty() && self.window_key_down.is_empty())
    }
}

/// A part of what a view shows: something that holds others ([`div`]),
/// text, a drawing ([`canvas`]) or another view. Built in `body`, styled
/// and given what it takes with the methods of [`Styled`],
/// [`ParentElement`] and [`InteractiveElement`].
pub struct Element {
    pub(crate) kind: Kind,
    pub(crate) style: StyleRefinement,
    pub(crate) key: Option<ElementId>,
    pub(crate) children: Vec<Element>,
    pub(crate) inter: Option<Box<Interactivity>>,
    /// What it holds is rows made as they come into view
    /// ([`uniform_rows`]).
    pub(crate) rows: Option<RowsSpec>,
}

/// Rows of one height, made as they come near what shows.
#[derive(Clone)]
pub(crate) struct RowsSpec {
    pub(crate) count: usize,
    pub(crate) row_height: Pixels,
    pub(crate) make: Rc<dyn Fn(std::ops::Range<usize>) -> Vec<Element>>,
}

/// `count` rows, each `row_height` tall, one under the other: only
/// those near what can be seen are in the window's tree. `make` is
/// asked for the elements of a run of rows (give each its row as its
/// id, `.id(row)`, so those that stay keep what they drew); each is
/// placed at its row. As what holds it scrolls, the window asks for
/// more, a run at a time with some to spare, without the view that
/// holds it saying what it shows again; the stage scrolls the ones
/// there with nothing drawn. A list of a hundred thousand rows costs
/// what a screenful does.
pub fn uniform_rows(count: usize, row_height: Pixels, make: impl Fn(std::ops::Range<usize>) -> Vec<Element> + 'static) -> Element {
    use crate::Styled as _;
    let mut element = div().relative().w_full().h(row_height * count as f32).flex_shrink_0();
    element.rows = Some(RowsSpec { count, row_height, make: Rc::new(make) });
    element
}

/// (The name GPUI had for an element of any kind: here there is one.)
pub type AnyElement = Element;

impl Element {
    pub(crate) fn new(kind: Kind) -> Element {
        Element { kind, style: StyleRefinement::default(), key: None, children: Vec::new(), inter: None, rows: None }
    }

    pub(crate) fn inter(&mut self) -> &mut Interactivity {
        self.inter.get_or_insert_with(Default::default)
    }

    /// [`Self::paint_key`], worked out from the view (of type `V`) when
    /// the element takes its place in the window's tree: for what's
    /// built where the view isn't to hand (a list's rows, keyed by what
    /// the view says each shows).
    pub fn key_by<V: 'static>(mut self, key: impl Fn(&V) -> u64 + 'static) -> Self {
        if let Kind::Canvas(canvas) = &mut self.kind {
            canvas.key_by = Some(Rc::new(move |view| view.downcast_ref::<V>().map_or(0, &key)));
        }
        self
    }

    /// What its drawing depends on (hash what it shows): with the same
    /// key, size and state as last time a [`canvas`] isn't drawn again
    /// when its view's `body` runs. Without one it's drawn again each
    /// time (the view said it changed, and a closure can't be compared).
    pub fn paint_key(mut self, key: u64) -> Self {
        if let Kind::Canvas(canvas) = &mut self.kind {
            canvas.key = Some(key);
        }
        self
    }

    /// Sizes a [`canvas`] by what it draws, given what's known of its
    /// size and the room it has.
    pub fn measured(mut self, measure: impl Fn(Size<Option<Pixels>>, Size<crate::AvailableSpace>, &mut crate::tree::Measuring) -> Size<Pixels> + 'static) -> Self {
        if let Kind::Canvas(canvas) = &mut self.kind {
            canvas.measure = Some(Rc::new(move |_, known, available, measuring| measure(known, available, measuring)));
        }
        self
    }
}

/// An element that holds others, laid out by its style (a row, a
/// column, a grid, a stack): what GPUI called a div, and Haiku a group.
pub fn div() -> Element {
    Element::new(Kind::Group)
}

/// A drawing: `paint` draws it, with a [`Painter`], in the bounds its
/// style gave it. It's kept, and drawn again only when the view's `body`
/// next runs (see [`Element::paint_key`]) or its size changes.
pub fn canvas(paint: impl Fn(&mut Painter, Bounds<Pixels>) + 'static) -> Element {
    Element::new(Kind::Canvas(CanvasSpec { paint: Rc::new(move |_, p, bounds, _| paint(p, bounds)), key: None, key_by: None, measure: None }))
}

/// A drawing by a view of type `V` (an element for its `body`, or for
/// what builds elements for it): `paint` is handed the view as it is
/// when the drawing is wanted. (`Cx::canvas`, without the view to hand.)
pub fn view_canvas<V: 'static>(paint: impl Fn(&V, &mut Painter, Bounds<Pixels>) + 'static) -> Element {
    Element::new(Kind::Canvas(CanvasSpec {
        paint: Rc::new(move |view, p, bounds, _| {
            if let Some(view) = view.downcast_ref::<V>() {
                paint(view, p, bounds)
            }
        }),
        key: None,
        key_by: None,
        measure: None,
    }))
}

/// A drawing that follows the pointer and the keyboard: `paint` is told
/// whether the pointer is over it, holding it down, and whether it has
/// the keyboard, and it's drawn again when any of those changes (only
/// it: nothing else in the window is touched).
pub fn control_canvas(paint: impl Fn(&mut Painter, Bounds<Pixels>, ControlState) + 'static) -> Element {
    let mut element = Element::new(Kind::Canvas(CanvasSpec { paint: Rc::new(move |_, p, bounds, state| paint(p, bounds, state)), key: None, key_by: None, measure: None }));
    element.inter().paints_by_state = true;
    element
}

/// Text of more than one look: `runs` say how much of it (in bytes) is
/// drawn how.
pub fn styled_text(text: impl Into<SharedString>, runs: Vec<TextRun>) -> Element {
    Element::new(Kind::Text(TextSpec { text: text.into(), runs: Some(runs) }))
}

impl Styled for Element {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Anything that can be put in a window: an [`Element`], text, a
/// widget (which makes an element of itself).
pub trait IntoElement: Sized {
    fn into_element(self) -> Element;

    /// (GPUI's name for the same.)
    fn into_any_element(self) -> Element {
        self.into_element()
    }
}

impl IntoElement for Element {
    fn into_element(self) -> Element {
        self
    }
}

impl IntoElement for &'static str {
    fn into_element(self) -> Element {
        Element::new(Kind::Text(TextSpec { text: self.into(), runs: None }))
    }
}

impl IntoElement for String {
    fn into_element(self) -> Element {
        Element::new(Kind::Text(TextSpec { text: self.into(), runs: None }))
    }
}

impl IntoElement for SharedString {
    fn into_element(self) -> Element {
        Element::new(Kind::Text(TextSpec { text: self, runs: None }))
    }
}

/// Elements that hold others.
pub trait ParentElement: Sized {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>);

    fn child(mut self, child: impl IntoElement) -> Self {
        self.extend(std::iter::once(child.into_element()));
        self
    }

    fn children(mut self, children: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.extend(children.into_iter().map(|child| child.into_element()));
        self
    }
}

impl ParentElement for Element {
    fn extend(&mut self, elements: impl IntoIterator<Item = Element>) {
        self.children.extend(elements);
    }
}

/// What an element takes of the pointer and the keyboard. Each handler
/// runs on the window's looper; a view's own are made with
/// `Cx::listener` (which hands it the view) or `Cx::msg` (which sends
/// the view a message).
pub trait InteractiveElement: Sized {
    #[doc(hidden)]
    fn interactivity(&mut self) -> &mut Interactivity;
    #[doc(hidden)]
    fn set_key(&mut self, key: ElementId);

    /// Tells it from its siblings when `body` runs again (see
    /// [`ElementId`]).
    fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.set_key(id.into());
        self
    }

    fn on_mouse_down(mut self, button: MouseButton, listener: impl Fn(&MouseDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().mouse_down.push((Some(button), Rc::new(listener)));
        self
    }

    fn on_any_mouse_down(mut self, listener: impl Fn(&MouseDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().mouse_down.push((None, Rc::new(listener)));
        self
    }

    fn on_mouse_up(mut self, button: MouseButton, listener: impl Fn(&MouseUpEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().mouse_up.push((Some(button), Rc::new(listener)));
        self
    }

    /// A release anywhere but over it (what was pressed here was let go
    /// of elsewhere).
    fn on_mouse_up_out(mut self, button: MouseButton, listener: impl Fn(&MouseUpEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().mouse_up_out.push((Some(button), Rc::new(listener)));
        self
    }

    fn on_mouse_move(mut self, listener: impl Fn(&MouseMoveEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().mouse_move.push(Rc::new(listener));
        self
    }

    fn on_scroll_wheel(mut self, listener: impl Fn(&ScrollWheelEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().scroll_wheel.push(Rc::new(listener));
        self
    }

    /// Files were dropped on it, dragged from another program (or
    /// another window): the topmost thing under the pointer that takes
    /// drops gets them, and nothing under it does. While they're over
    /// the window it's hovered as under the pointer (`on_hover`, its
    /// hover style), so what would take them can show it.
    fn on_drop(mut self, listener: impl Fn(&crate::ExternalPaths, &mut WindowCx) + 'static) -> Self {
        self.interactivity().file_drop.push(Rc::new(listener));
        self
    }

    /// Pressed and released on it (or Space while it has the keyboard).
    fn on_click(mut self, listener: impl Fn(&ClickEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().click.push(Rc::new(listener));
        self
    }

    /// The pointer came over it (true) or left.
    fn on_hover(mut self, listener: impl Fn(&bool, &mut WindowCx) + 'static) -> Self {
        self.interactivity().hover.push(Rc::new(listener));
        self
    }

    /// A key, while it or something in it has the keyboard.
    fn on_key_down(mut self, listener: impl Fn(&KeyDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().key_down.push(Rc::new(listener));
        self
    }

    fn on_key_up(mut self, listener: impl Fn(&KeyUpEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().key_up.push(Rc::new(listener));
        self
    }

    /// A key on its way to what has the keyboard, before that takes it.
    fn capture_key_down(mut self, listener: impl Fn(&KeyDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().capture_key_down.push(Rc::new(listener));
        self
    }

    fn on_modifiers_changed(mut self, listener: impl Fn(&ModifiersChangedEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().modifiers_changed.push(Rc::new(listener));
        self
    }

    /// A press anywhere in the window, over this or not.
    fn on_window_mouse_down(mut self, listener: impl Fn(&MouseDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().window_mouse_down.push(Rc::new(listener));
        self
    }

    /// A release anywhere in the window.
    fn on_window_mouse_up(mut self, listener: impl Fn(&MouseUpEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().window_mouse_up.push(Rc::new(listener));
        self
    }

    /// The pointer moving anywhere in the window (what's dragged from
    /// here follows it).
    fn on_window_mouse_move(mut self, listener: impl Fn(&MouseMoveEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().window_mouse_move.push(Rc::new(listener));
        self
    }

    /// It takes typing while it (or something in it) has the keyboard:
    /// what keys type and what an input method composes. A view's own:
    /// `Cx::text_input`.
    fn on_text_input(mut self, listener: impl Fn(&TextInputEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().text_input = Some(Rc::new(listener));
        self
    }

    /// A key nothing with the keyboard took, wherever the keyboard is in
    /// the window (after what has it and everything that holds it; the
    /// first such element in the window to stop it has it): the keys a
    /// part of the window answers for all of it (a tab view's Next and
    /// Previous Tab).
    fn on_window_key_down(mut self, listener: impl Fn(&KeyDownEvent, &mut WindowCx) + 'static) -> Self {
        self.interactivity().window_key_down.push(Rc::new(listener));
        self
    }

    /// The keyboard came to it, or into it (to something it holds), or
    /// left it: true while it's there.
    fn on_focus_changed(mut self, listener: impl Fn(&bool, &mut WindowCx) + 'static) -> Self {
        self.interactivity().focus_changed.push(Rc::new(listener));
        self
    }

    /// Its style while the pointer is over it.
    fn hover(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.interactivity().hover_style = Some(Box::new(f(StyleRefinement::default())));
        self
    }

    /// Its style while it's held down.
    fn active(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.interactivity().active_style = Some(Box::new(f(StyleRefinement::default())));
        self
    }

    /// Its style while it has the keyboard.
    fn focus(mut self, f: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        self.interactivity().focus_style = Some(Box::new(f(StyleRefinement::default())));
        self
    }

    /// What's under it gets nothing of the pointer (a sheet over a
    /// window's content).
    fn occlude(mut self) -> Self {
        self.interactivity().occlude = true;
        self
    }

    /// Drawn over everything else in the window, wherever it is in the
    /// tree, cut by nothing but the window, and first under the pointer:
    /// a list that drops from a field over what's below it (a search's
    /// suggestions). Laid out where it is as anything else. With
    /// [`Self::occlude`] nothing under it gets the pointer.
    fn raised(mut self) -> Self {
        self.interactivity().raised = true;
        self
    }

    /// It can have the keyboard, and a click on it gives it (something
    /// typed into; `Cx::focus` gives it too).
    fn focusable(mut self) -> Self {
        let inter = self.interactivity();
        inter.focusable = true;
        inter.click_focus = true;
        self
    }

    /// In the window's tab order: Tab and ⇧Tab move the keyboard to it.
    /// (A click doesn't, as BeOS's controls: pressing a button leaves
    /// the keyboard where it was.)
    fn tab_stop(mut self) -> Self {
        let inter = self.interactivity();
        inter.focusable = true;
        inter.tab_stop = true;
        self
    }

    /// The scroller this element is, when its style says it scrolls
    /// (`overflow_y_scroll`): kept by its view, to know where it is and
    /// to move it. (Without one it keeps its own.)
    fn track_scroll(mut self, handle: &crate::ScrollHandle) -> Self {
        self.interactivity().scroll = Some(handle.clone());
        self
    }

    /// Hears where it's scrolled to (its offset) whenever that changes,
    /// whoever moved it.
    fn on_scroll(mut self, listener: impl Fn(&Point<Pixels>, &mut WindowCx) + 'static) -> Self {
        self.interactivity().scroll_listeners.push(Rc::new(listener));
        self
    }

    /// Hears where it is (in the window) and how big, whenever layout
    /// changes either: for what follows the size of what it holds (a
    /// window that fits its content). Told once layout is done, before
    /// anything is drawn; what it changes is laid out in the same turn.
    fn on_bounds_changed(mut self, listener: impl Fn(&Bounds<Pixels>, &mut WindowCx) + 'static) -> Self {
        self.interactivity().bounds_listeners.push(Rc::new(listener));
        self
    }

    /// It's drawn again whenever `handle`'s scroller moves or its ends
    /// do (a scroll bar's knob follows its view).
    fn watch_scroll(mut self, handle: &crate::ScrollHandle) -> Self {
        self.interactivity().watches.push((handle.id(), crate::scroll::ANY));
        self
    }

    /// It's drawn again whenever `handle`'s scroller moves across (a
    /// table's header, over its rows), not when it moves up or down.
    fn watch_scroll_x(mut self, handle: &crate::ScrollHandle) -> Self {
        self.interactivity().watches.push((handle.id(), crate::scroll::ACROSS | crate::scroll::ENDS));
        self
    }

    /// It's drawn again whenever how far `handle`'s scroller goes, or
    /// its view, changes: not for its moving (a scroll bar's arrows,
    /// grey with nothing to scroll).
    fn watch_scroll_ends(mut self, handle: &crate::ScrollHandle) -> Self {
        self.interactivity().watches.push((handle.id(), crate::scroll::ENDS));
        self
    }

    /// A text that agents aren't told of as a label of its own: what
    /// holds it says what it is (a document, read whole as its value).
    fn unlabelled(mut self) -> Self {
        self.interactivity().unlabelled = true;
        self
    }

    /// It can have the keyboard (a click gives it), and `handle` is how
    /// its view gives it the keyboard and knows whether it has it.
    fn track_focus(mut self, handle: &crate::FocusHandle) -> Self {
        let inter = self.interactivity();
        inter.focusable = true;
        inter.click_focus = true;
        inter.focus_handle = Some(handle.id());
        self
    }

    /// It counts as having the keyboard while anything it holds has it:
    /// its focus style applies and it draws focused (a text control's
    /// well around its field).
    fn focus_within(mut self) -> Self {
        self.interactivity().focus_within = true;
        self
    }

    /// Return clicks it from anywhere in its window, when what has the
    /// keyboard doesn't take Return itself (a text field's Return is its
    /// own): BeOS's default button.
    fn default_key(mut self) -> Self {
        self.interactivity().default_key = true;
        self
    }

    /// It can't be used now: it takes nothing.
    fn disabled(mut self, disabled: bool) -> Self {
        self.interactivity().disabled = disabled;
        self
    }

    /// What agents read it as: its role (AccessKit's: "Button",
    /// "CheckBox", "TextInput"…) and name.
    fn agent(mut self, role: &'static str, name: impl Into<SharedString>) -> Self {
        let agent = self.interactivity().agent.get_or_insert_with(Default::default);
        agent.role = role;
        agent.name = Some(name.into());
        self
    }

    /// What agents read as its value (a field's text, a menu's choice).
    fn agent_value(mut self, value: impl Into<SharedString>) -> Self {
        self.interactivity().agent.get_or_insert_with(Default::default).value = Some(value.into());
        self
    }

    /// Whether agents read it as checked (check boxes, radio buttons).
    fn agent_checked(mut self, checked: Option<bool>) -> Self {
        self.interactivity().agent.get_or_insert_with(Default::default).checked = checked;
        self
    }

    /// Whether agents read it as taking clicks (otherwise: whether it
    /// has a handler for one).
    fn agent_clickable(mut self, clickable: bool) -> Self {
        self.interactivity().agent.get_or_insert_with(Default::default).clickable = Some(clickable);
        self
    }

    /// Whether agents read it as selected (a list's row, a tab).
    fn agent_selected(mut self, selected: Option<bool>) -> Self {
        self.interactivity().agent.get_or_insert_with(Default::default).selected = selected;
        self
    }
}

impl InteractiveElement for Element {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.inter()
    }

    fn set_key(&mut self, key: ElementId) {
        self.key = Some(key);
    }
}
