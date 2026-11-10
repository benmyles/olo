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
    Bounds, ClickEvent, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Painter, Pixels, ScrollWheelEvent, SharedString, Size,
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

pub(crate) struct CanvasSpec {
    pub(crate) paint: PaintFn,
    /// What the drawing depends on, if the element says: the same key as
    /// last time (and the same size and state), it isn't drawn again when
    /// its view's `body` runs.
    pub(crate) key: Option<u64>,
    pub(crate) measure: Option<MeasureFn>,
}

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
    pub(crate) focus_changed: SmallVec<[Listener<bool>; 1]>,
    pub(crate) hover_style: Option<Box<StyleRefinement>>,
    pub(crate) active_style: Option<Box<StyleRefinement>>,
    pub(crate) focus_style: Option<Box<StyleRefinement>>,
    /// What's under it gets nothing of the pointer.
    pub(crate) occlude: bool,
    /// It can have the keyboard.
    pub(crate) focusable: bool,
    /// Tab reaches it.
    pub(crate) tab_stop: bool,
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
            || !(self.mouse_down.is_empty() && self.mouse_up.is_empty() && self.mouse_move.is_empty() && self.scroll_wheel.is_empty() && self.click.is_empty() && self.hover.is_empty())
    }

    pub(crate) fn hears_window(&self) -> bool {
        self.default_key || !(self.window_mouse_down.is_empty() && self.window_mouse_up.is_empty() && self.window_mouse_move.is_empty() && self.mouse_up_out.is_empty())
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
}

/// (The name GPUI had for an element of any kind: here there is one.)
pub type AnyElement = Element;

impl Element {
    pub(crate) fn new(kind: Kind) -> Element {
        Element { kind, style: StyleRefinement::default(), key: None, children: Vec::new(), inter: None }
    }

    pub(crate) fn inter(&mut self) -> &mut Interactivity {
        self.inter.get_or_insert_with(Default::default)
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
    Element::new(Kind::Canvas(CanvasSpec { paint: Rc::new(move |_, p, bounds, _| paint(p, bounds)), key: None, measure: None }))
}

/// A drawing that follows the pointer and the keyboard: `paint` is told
/// whether the pointer is over it, holding it down, and whether it has
/// the keyboard, and it's drawn again when any of those changes (only
/// it: nothing else in the window is touched).
pub fn control_canvas(paint: impl Fn(&mut Painter, Bounds<Pixels>, ControlState) + 'static) -> Element {
    let mut element = Element::new(Kind::Canvas(CanvasSpec { paint: Rc::new(move |_, p, bounds, state| paint(p, bounds, state)), key: None, measure: None }));
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

    /// The keyboard came to it (true) or left.
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

    /// It can have the keyboard (a click gives it; `Cx::focus` too).
    fn focusable(mut self) -> Self {
        self.interactivity().focusable = true;
        self
    }

    /// In the window's tab order: Tab and ⇧Tab move the keyboard to it.
    fn tab_stop(mut self) -> Self {
        let inter = self.interactivity();
        inter.focusable = true;
        inter.tab_stop = true;
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
