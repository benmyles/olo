//! A part of a window drawn from the app's state and cached
//! (`AnyView::cached`): drawn again as the app changes (or another model
//! it reads, [`Part::observe`]), its last frame kept otherwise. GPUI
//! draws again every view above one that changed, so a window made of
//! parts redraws only the part that changed and its few boxes around:
//! a one-time code counting down in Passwords' item pane, a graph moving
//! in Activity Monitor's, leave the rest of the window as it was.

use gpui::{AnyElement, AnyView, Context, Entity, IntoElement, Render, StyleRefinement, Subscription, Window};

pub struct Part<T: 'static> {
    app: Entity<T>,
    draw: fn(&mut T, &mut Window, &mut Context<T>) -> AnyElement,
    _observe: Vec<Subscription>,
}

impl<T: 'static> Part<T> {
    /// `draw` draws it from the app.
    pub fn new(app: &Entity<T>, draw: fn(&mut T, &mut Window, &mut Context<T>) -> AnyElement, cx: &mut Context<Self>) -> Part<T> {
        Part { app: app.clone(), draw, _observe: vec![cx.observe(app, |_, _, cx| cx.notify())] }
    }

    /// Drawn again as `other` changes too (a list's model it shows: a
    /// cached view isn't drawn again for a model it only reads).
    pub fn observe<E: 'static>(mut self, other: &Entity<E>, cx: &mut Context<Self>) -> Self {
        self._observe.push(cx.observe(other, |_, _, cx| cx.notify()));
        self
    }
}

impl<T: 'static> Render for Part<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let draw = self.draw;
        self.app.update(cx, |a, cx| draw(a, window, cx))
    }
}

/// `part` in its place, `style` its size (a cached view is laid out from
/// its style, not what it draws).
pub fn cached<T: 'static>(part: &Entity<Part<T>>, style: StyleRefinement) -> AnyView {
    AnyView::from(part.clone()).cached(style)
}
