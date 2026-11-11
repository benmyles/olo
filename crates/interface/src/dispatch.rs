//! Input: who gets the pointer and the keyboard.
//!
//! The pointer is over every node that holds it, topmost first, down to
//! the first that takes it all ([`crate::InteractiveElement::occlude`]).
//! Each of those hears what the pointer does, from the top, until one
//! says it goes no further. The keyboard belongs to one node (the
//! focus); a key goes down to it first (`capture_key_down`), then up
//! from it through what holds it.
//!
//! What follows the pointer keeps its state in its node: the pointer
//! coming over a control, or a press on it, draws that control again
//! and nothing else. No view's `body` runs for it.

use smallvec::SmallVec;

use crate::{
    Bounds, ClickEvent, ImeInput, KeyDownEvent, KeyboardButton, KeyboardClickEvent, MouseButton, MouseClickEvent, MouseDownEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollWheelEvent,
    WindowCx,
    element::{Interactivity, Listener},
    tree::{NodeId, NodeKind},
    window::WindowCore,
};

impl WindowCore {
    /// What's under a point: every node that takes the pointer there,
    /// topmost first, down to the first that takes it all.
    fn nodes_at(&self, at: Point<Pixels>) -> Vec<NodeId> {
        let mut found = Vec::new();
        if let Some(root) = self.root {
            let window = Bounds { origin: Point::default(), size: self.size };
            // (What's raised is over the rest: gathered after it.)
            let mut raised = Vec::new();
            self.gather(root, at, window, &mut found, Some(&mut raised));
            for (id, at) in raised {
                self.gather(id, at, window, &mut found, None);
            }
        }
        // (Gathered in painting order: what's painted last is on top.)
        found.reverse();
        if let Some(stop) = found.iter().position(|id| self.nodes[*id].inter.as_ref().is_some_and(|i| i.occlude)) {
            found.truncate(stop + 1);
        }
        found
    }

    fn gather(&self, id: NodeId, at: Point<Pixels>, clip: Bounds<Pixels>, found: &mut Vec<NodeId>, mut raised: Option<&mut Vec<(NodeId, Point<Pixels>)>>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden || !node.reach.contains(&at) {
            return;
        }
        if node.inter.as_ref().is_some_and(|i| i.raised)
            && let Some(raised) = raised.as_deref_mut()
        {
            raised.push((id, at));
            return;
        }
        if node.bounds.contains(&at)
            && clip.contains(&at)
            && (node.style.mouse_cursor.is_some() || node.inter.as_ref().is_some_and(|i| i.takes_pointer()) || node.notes.as_ref().is_some_and(|n| !n.cursors.is_empty()))
            && !node.inter.as_ref().is_some_and(|i| i.disabled)
        {
            found.push(id);
        }
        if node.children.is_empty() {
            return;
        }
        let inner = match node.style.overflow_mask(node.bounds, self.gfx.rem_size, self.gfx.scale_factor) {
            Some(mask) => clip.intersect(&mask.bounds),
            None => clip,
        };
        if !inner.contains(&at) {
            // (Nothing it holds shows there.)
            return;
        }
        // What it holds is where it's laid out, shown from wherever
        // it's scrolled to: the pointer is over what's that far on.
        let (at, inner) = match &node.scroll {
            Some(handle) => {
                let shown = handle.0.shown.get();
                (crate::point(at.x - shown.x, at.y - shown.y), Bounds { origin: crate::point(crate::px(-1.0e7), crate::px(-1.0e7)), size: crate::size(crate::px(2.0e7), crate::px(2.0e7)) })
            }
            None => (at, inner),
        };
        for child in &node.children {
            self.gather(*child, at, inner, found, raised.as_deref_mut());
        }
    }

    /// A node's look follows how the pointer and keyboard stand to it:
    /// its style if it has one for that, its drawing if it draws by it.
    pub(crate) fn state_changed(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get_mut(id) else { return };
        let Some(inter) = &node.inter else { return };
        let styled = inter.hover_style.is_some() || inter.active_style.is_some() || inter.focus_style.is_some();
        let by_state = inter.paints_by_state;
        if styled {
            let mut style = crate::Style::default();
            use refineable::Refineable as _;
            style.refine(&node.refinement);
            if node.focused
                && let Some(s) = &inter.focus_style
            {
                style.refine(s);
            }
            if node.hovered
                && let Some(s) = &inter.hover_style
            {
                style.refine(s);
            }
            if node.pressed
                && node.hovered
                && let Some(s) = &inter.active_style
            {
                style.refine(s);
            }
            if style != node.style {
                let subtree = style.text != node.style.text || style.opacity != node.style.opacity;
                node.style = style;
                let layout = node.layout;
                let (rem, scale) = (self.gfx.rem_size, self.gfx.scale_factor);
                self.layout.set_style(layout, &self.nodes[id].style, rem, scale);
                if subtree {
                    self.mark_subtree(id, true);
                } else {
                    self.mark_paint(id);
                }
            }
        }
        if by_state {
            self.mark_paint(id);
        }
    }

    /// Everything Tab reaches under a node, in the order it's drawn.
    pub(crate) fn tab_stops(&self, id: NodeId, stops: &mut Vec<NodeId>) {
        let Some(node) = self.nodes.get(id) else { return };
        if node.style.visibility == crate::Visibility::Hidden {
            return;
        }
        if node.inter.as_ref().is_some_and(|i| i.tab_stop && !i.disabled) {
            stops.push(id);
        }
        for child in &node.children {
            self.tab_stops(*child, stops);
        }
    }

    /// Where keys go while nothing has the keyboard: the window's root
    /// and the element its first view shows (which takes the window's
    /// own keys: its menus').
    fn unfocused_path(&self) -> SmallVec<[NodeId; 16]> {
        let mut path: SmallVec<[NodeId; 16]> = SmallVec::new();
        if let Some(root) = self.root {
            path.push(root);
            if let Some(first) = self.nodes.get(root).and_then(|n| n.children.first().copied()) {
                path.push(first);
            }
        }
        path
    }

    /// The path from the window's root down to a node.
    fn path_to(&self, id: NodeId) -> SmallVec<[NodeId; 16]> {
        let mut path: SmallVec<[NodeId; 16]> = SmallVec::new();
        let mut at = Some(id);
        while let Some(id) = at {
            path.push(id);
            at = self.nodes.get(id).and_then(|n| n.parent);
        }
        path.reverse();
        path
    }
}

impl WindowCx<'_> {
    pub(crate) fn dispatch_input(&mut self, input: PlatformInput) {
        self.core.propagate = true;
        self.core.default_prevented = false;
        // Whether the pointer is over the window: what's laid out under
        // where it last was (or under the corner, before it ever came:
        // a new Deskbar's Olo tile was lit until the pointer first
        // moved over it) isn't under the pointer.
        match &input {
            PlatformInput::MouseMove(_) | PlatformInput::MouseDown(_) | PlatformInput::MouseUp(_) | PlatformInput::ScrollWheel(_) => self.core.pointer_in = true,
            PlatformInput::FileDrop(crate::FileDropEvent::Entered { .. } | crate::FileDropEvent::Pending { .. } | crate::FileDropEvent::Submit { .. }) => self.core.pointer_in = true,
            PlatformInput::MouseExited(_) | PlatformInput::FileDrop(crate::FileDropEvent::Exited) => self.core.pointer_in = false,
            _ => {}
        }
        match input {
            PlatformInput::MouseMove(event) => {
                self.core.mouse = event.position;
                self.core.modifiers = event.modifiers;
                self.update_hover();
                self.to_window_listeners(|i| &i.window_mouse_move, &event);
                self.to_hovered(|i| i.mouse_move.iter().cloned().collect(), &event);
            }
            PlatformInput::MouseDown(event) => self.mouse_down(event),
            PlatformInput::MouseUp(event) => self.mouse_up(event),
            // (A wheel the stage left alone: something under the
            // pointer takes it itself, or nothing there scrolls.)
            PlatformInput::ScrollWheel(event) => {
                self.core.mouse = event.position;
                self.core.modifiers = event.modifiers;
                self.update_hover();
                self.to_hovered(|i| i.scroll_wheel.iter().cloned().collect(), &event);
                if self.core.propagate && !self.core.default_prevented && !matches!(event.touch_phase, crate::TouchPhase::Ended) {
                    self.scroll_under_pointer(&event);
                }
            }
            PlatformInput::MouseExited(_) => self.pointer_left(),
            PlatformInput::KeyDown(event) => self.key_down(event),
            PlatformInput::KeyUp(event) => {
                self.core.modifiers = event.keystroke.modifiers;
                self.to_focus_path(|i| i.key_up.iter().cloned().collect(), &event);
            }
            PlatformInput::ModifiersChanged(event) => {
                self.core.modifiers = event.modifiers;
                self.to_focus_path(|i| i.modifiers_changed.iter().cloned().collect(), &event);
                // (A key held changes how a drag would be taken: ⌥ copies.)
                self.ask_drop();
            }
            PlatformInput::FileDrop(event) => self.file_drop(event),
            PlatformInput::Pinch(event) => {
                self.core.mouse = event.position;
                self.core.modifiers = event.modifiers;
                self.core.pointer_in = true;
                self.update_hover();
                self.to_hovered(|i| i.pinch.iter().cloned().collect(), &event);
            }
        }
    }

    /// Files dragged over the window from somewhere else: what's under
    /// them is hovered as under the pointer, and dropped, they go to
    /// the topmost thing there that takes drops (`on_drop`).
    fn file_drop(&mut self, event: crate::FileDropEvent) {
        use crate::FileDropEvent;
        match event {
            FileDropEvent::Entered { position, paths } => {
                self.core.dropping = Some(paths);
                self.core.mouse = position;
                self.update_hover();
                self.ask_drop();
            }
            FileDropEvent::Pending { position } => {
                self.core.mouse = position;
                self.update_hover();
                self.ask_drop();
            }
            FileDropEvent::Submit { position } => {
                self.core.mouse = position;
                self.update_hover();
                self.end_drop();
                let Some(paths) = self.core.dropping.take() else { return };
                for id in self.core.hovered.clone() {
                    let listeners: SmallVec<[Listener<crate::ExternalPaths>; 1]> =
                        self.core.nodes.get(id).and_then(|n| n.inter.as_ref()).filter(|i| !i.disabled).map(|i| i.file_drop.clone()).unwrap_or_default();
                    if listeners.is_empty() {
                        continue;
                    }
                    self.core.current = Some(id);
                    for listener in listeners {
                        listener(&paths, self);
                    }
                    self.core.current = None;
                    break;
                }
            }
            FileDropEvent::Exited => {
                self.core.dropping = None;
                self.end_drop();
                self.pointer_left();
            }
            // What's dragged over it, whatever it is: the window's own
            // view says where it would take it (`View::drop_over`).
            FileDropEvent::Offered(offer) => {
                self.core.drop_offer = Some(offer);
                self.core.drop_accept_sent = None;
                self.ask_drop();
            }
            // Dropped where the view took it, its data read.
            FileDropEvent::Dropped(dropped) => {
                self.end_drop();
                if let Some(view) = self.core.main_view.or(self.core.root_view) {
                    self.with_any_view(view, |v, id, cx| v.dropped(id, dropped, cx));
                }
            }
        }
    }

    /// Asks the window's own view where it would take what's dragged
    /// over it, and tells the display thread when that changes (what
    /// the pointer shows, and what the source is told).
    pub(crate) fn ask_drop(&mut self) {
        let Some(offer) = self.core.drop_offer.clone() else { return };
        let Some(view) = self.core.main_view.or(self.core.root_view) else { return };
        let at = self.core.mouse;
        let Some(accept) = self.with_any_view(view, |v, id, cx| v.takes_drops().then(|| v.drop_over(id, Some(&offer), at, cx))).flatten() else { return };
        if self.core.drop_accept_sent.as_ref() != Some(&accept) {
            self.core.drop_accept_sent = Some(accept.clone());
            self.on_display(move |window| window.set_drop_accept(accept));
        }
    }

    /// What's dragged over the window now, if anything.
    pub fn drop_offer(&self) -> Option<&crate::DropOffer> {
        self.core.drop_offer.as_ref()
    }

    /// Asks the window's own view again where it would take what's
    /// dragged over it: what's under the pointer changed with the
    /// pointer still (it scrolled, a folder sprang open).
    pub fn reconsider_drop(&mut self) {
        self.ask_drop();
    }

    /// The drag has left or been dropped: the view is told nothing is
    /// over it.
    fn end_drop(&mut self) {
        if self.core.drop_offer.take().is_none() {
            return;
        }
        self.core.drop_accept_sent = None;
        let at = self.core.mouse;
        if let Some(view) = self.core.main_view.or(self.core.root_view) {
            self.with_any_view(view, |v, id, cx| {
                if v.takes_drops() {
                    v.drop_over(id, None, at, cx);
                }
            });
        }
    }

    /// What took the wheel let it go on: the nearest thing round it
    /// that scrolls that way scrolls (stopping at its ends).
    fn scroll_under_pointer(&mut self, event: &ScrollWheelEvent) {
        let delta = event.delta.pixel_delta(crate::px(20.));
        let mut at = self.core.hovered.first().copied();
        while let Some(id) = at {
            let Some(node) = self.core.nodes.get(id) else { break };
            at = node.parent;
            let Some(handle) = node.scroll.clone() else { continue };
            let max = handle.max_offset();
            let by = crate::point(if max.width > crate::px(0.) { delta.x } else { crate::px(0.) }, if max.height > crate::px(0.) { delta.y } else { crate::px(0.) });
            if by != Point::default() {
                handle.scroll_by(self, by);
                break;
            }
        }
    }

    /// What an input method did goes to what takes typing where the
    /// keyboard is.
    pub(crate) fn dispatch_ime(&mut self, ime: ImeInput) {
        let event = match ime {
            ImeInput::InsertText(text) => crate::TextInputEvent::Insert(text),
            ImeInput::SetMarkedText(text) => crate::TextInputEvent::Mark(text),
            ImeInput::UnmarkText => crate::TextInputEvent::Unmark,
            ImeInput::DeleteText => crate::TextInputEvent::DeleteMarked,
        };
        self.to_text_input(&event);
    }

    /// Hands typing to the nearest thing that takes it, from what has
    /// the keyboard up. Whether anything did.
    fn to_text_input(&mut self, event: &crate::TextInputEvent) -> bool {
        let Some(focus) = self.core.focus else { return false };
        let path = self.core.path_to(focus);
        for id in path.iter().rev() {
            let handler = self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).filter(|i| !i.disabled).and_then(|i| i.text_input.clone());
            if let Some(handler) = handler {
                self.core.current = Some(*id);
                handler(event, self);
                self.core.current = None;
                return true;
            }
        }
        false
    }

    /// Works out what the pointer is over now, and tells what it came
    /// over or left.
    pub(crate) fn update_hover(&mut self) {
        let now = if self.core.pointer_in { self.core.nodes_at(self.core.mouse) } else { Vec::new() };
        if now != self.core.hovered {
            let before = std::mem::replace(&mut self.core.hovered, now.clone());
            for id in before.iter().filter(|id| !now.contains(id)) {
                self.set_hovered(*id, false);
            }
            for id in now.iter().filter(|id| !before.contains(id)) {
                self.set_hovered(*id, true);
            }
        }
        // The pointer's shape: the topmost thing under it that says
        // one, for where in it the pointer is (its drawing may have
        // said: a table's column dividers) or for all of it.
        let at = self.core.mouse;
        let cursor = now
            .iter()
            .find_map(|id| {
                let node = self.core.nodes.get(*id)?;
                let noted = node.notes.as_ref().filter(|n| !n.cursors.is_empty()).and_then(|notes| {
                    let shift = self.core.shift_at(*id);
                    let moved = crate::point(node.bounds.origin.x - node.painted_at.x + shift.x, node.bounds.origin.y - node.painted_at.y + shift.y);
                    let at = crate::point(at.x - moved.x, at.y - moved.y);
                    notes.cursors.iter().rev().find(|(b, _)| b.contains(&at)).map(|(_, style)| *style)
                });
                noted.or(node.style.mouse_cursor)
            })
            .unwrap_or(crate::CursorStyle::Arrow);
        self.set_cursor(cursor);
    }

    fn set_hovered(&mut self, id: NodeId, hovered: bool) {
        let Some(node) = self.core.nodes.get_mut(id) else { return };
        if node.hovered == hovered {
            return;
        }
        node.hovered = hovered;
        let listeners: SmallVec<[Listener<bool>; 1]> = node.inter.as_ref().map(|i| i.hover.clone()).unwrap_or_default();
        self.core.state_changed(id);
        // (It's the handlers' element: where it is, `listener_bounds`,
        // is where its tip goes. Within another's handler, as when a
        // click moves what's under the pointer, that one's again after.)
        let outer = self.core.current;
        for listener in listeners {
            self.core.current = Some(id);
            listener(&hovered, self);
        }
        self.core.current = outer;
    }

    /// The pointer left the window: it's over nothing.
    pub(crate) fn pointer_left(&mut self) {
        for id in std::mem::take(&mut self.core.hovered) {
            self.set_hovered(id, false);
        }
    }

    /// The pointer's shape, as the display thread shows it.
    fn set_cursor(&mut self, cursor: crate::CursorStyle) {
        if self.core.cursor == cursor {
            return;
        }
        self.core.cursor = cursor;
        crate::display::display().post(move |on| {
            if let Some(client) = on.client.wayland() {
                crate::LinuxClient::set_cursor_style(client, cursor);
            }
        });
    }

    fn mouse_down(&mut self, event: MouseDownEvent) {
        self.core.mouse = event.position;
        self.core.modifiers = event.modifiers;
        self.update_hover();
        self.to_window_listeners(|i| &i.window_mouse_down, &event);
        let hovered = self.core.hovered.clone();
        // What it went down on is held until it's let go: a click is
        // both on the same thing.
        let mut focus = None;
        let mut kept = false;
        for id in &hovered {
            let Some(node) = self.core.nodes.get_mut(*id) else { continue };
            let Some(inter) = &node.inter else { continue };
            // (What's in front decides: it keeps the keyboard where it
            // is, or takes it.)
            kept |= focus.is_none() && inter.keeps_focus;
            if focus.is_none() && !kept && inter.click_focus {
                focus = Some(*id);
            }
            if event.button == MouseButton::Left && (inter.paints_by_state || inter.active_style.is_some() || !inter.click.is_empty()) {
                node.pressed = true;
                self.core.pressed.push((*id, event.clone()));
                self.core.state_changed(*id);
            } else if !inter.click.is_empty() {
                self.core.pressed.push((*id, event.clone()));
            }
        }
        if let Some(focus) = focus {
            self.focus_node(Some(focus));
        }
        let button = event.button;
        self.to_hovered(|i| i.mouse_down.iter().filter(|(b, _)| b.is_none_or(|b| b == button)).map(|(_, l)| l.clone()).collect(), &event);
    }

    fn mouse_up(&mut self, event: MouseUpEvent) {
        self.core.mouse = event.position;
        self.core.modifiers = event.modifiers;
        self.update_hover();
        self.to_window_listeners(|i| &i.window_mouse_up, &event);
        let button = event.button;
        // A release anywhere but over them, for those that asked.
        let outside: Vec<Listener<MouseUpEvent>> = self
            .core
            .window_listeners
            .iter()
            .filter(|id| !self.core.hovered.contains(id))
            .filter_map(|id| self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()))
            .flat_map(|i| i.mouse_up_out.iter().filter(|(b, _)| b.is_none_or(|b| b == button)).map(|(_, l)| l.clone()))
            .collect();
        for listener in outside {
            listener(&event, self);
        }
        self.core.propagate = true;
        self.to_hovered(|i| i.mouse_up.iter().filter(|(b, _)| b.is_none_or(|b| b == button)).map(|(_, l)| l.clone()).collect(), &event);
        // Let go: what was held is no longer; pressed and released on
        // the same thing is a click.
        let held: Vec<(NodeId, MouseDownEvent)> = {
            let (held, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.core.pressed).into_iter().partition(|(_, down)| down.button == button);
            self.core.pressed = kept;
            held
        };
        self.core.propagate = true;
        for (id, down) in held {
            let Some(node) = self.core.nodes.get_mut(id) else { continue };
            if node.pressed {
                node.pressed = false;
                self.core.state_changed(id);
            }
            if !self.core.hovered.contains(&id) || !self.core.propagate {
                continue;
            }
            let listeners: SmallVec<[Listener<ClickEvent>; 1]> = self.core.nodes.get(id).and_then(|n| n.inter.as_ref()).map(|i| i.click.clone()).unwrap_or_default();
            let click = ClickEvent::Mouse(MouseClickEvent { down, up: event.clone() });
            self.core.current = Some(id);
            for listener in listeners {
                listener(&click, self);
            }
            self.core.current = None;
        }
    }

    fn key_down(&mut self, event: KeyDownEvent) {
        self.core.modifiers = event.keystroke.modifiers;
        let path = self.core.focus.map(|f| self.core.path_to(f)).unwrap_or_else(|| self.core.unfocused_path());
        // Down to what has the keyboard, for those that take keys first.
        for id in &path {
            let listeners: SmallVec<[Listener<KeyDownEvent>; 1]> = self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).map(|i| i.capture_key_down.clone()).unwrap_or_default();
            for listener in listeners {
                listener(&event, self);
                if !self.core.propagate {
                    return;
                }
            }
        }
        // Then up from it.
        for id in path.iter().rev() {
            let listeners: SmallVec<[Listener<KeyDownEvent>; 1]> = self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).map(|i| i.key_down.clone()).unwrap_or_default();
            self.core.current = Some(*id);
            for listener in listeners {
                listener(&event, self);
                if !self.core.propagate {
                    self.core.current = None;
                    return;
                }
            }
            self.core.current = None;
            // Space clicks what has the keyboard, if it takes clicks.
            if Some(*id) == self.core.focus && event.keystroke.key == "space" && !event.keystroke.modifiers.modified() && self.keyboard_click(*id, KeyboardButton::Space) {
                return;
            }
        }
        // Nothing with the keyboard took it: what answers for the whole
        // window may (a tab view's keys).
        let hearing: Vec<(NodeId, Listener<KeyDownEvent>)> = self
            .core
            .window_listeners
            .iter()
            .filter(|id| self.core.is_attached(**id))
            .filter_map(|id| Some((*id, self.core.nodes.get(*id)?.inter.as_ref()?)))
            .flat_map(|(id, i)| i.window_key_down.iter().cloned().map(move |l| (id, l)))
            .collect();
        for (id, listener) in hearing {
            self.core.current = Some(id);
            listener(&event, self);
            self.core.current = None;
            if !self.core.propagate {
                return;
            }
        }
        // Tab moves the keyboard on; Return clicks the window's default
        // button.
        let key = event.keystroke.key.as_str();
        let plain = !event.keystroke.modifiers.control && !event.keystroke.modifiers.alt && !event.keystroke.modifiers.platform;
        if key == "tab" && plain {
            self.focus_next(event.keystroke.modifiers.shift);
            return;
        } else if key == "enter" && !event.keystroke.modifiers.modified() {
            let default = self
                .core
                .window_listeners
                .iter()
                .copied()
                .find(|id| self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).is_some_and(|i| i.default_key && !i.disabled) && self.core.is_attached(*id));
            if let Some(default) = default {
                self.keyboard_click(default, KeyboardButton::Enter);
                return;
            }
        }
        // Nothing took it as a key: what it types goes into the text
        // where the keyboard is.
        if event.keystroke.modifiers.is_subset_of(&crate::Modifiers::shift())
            && let Some(typed) = event.keystroke.key_char.clone()
            && self.to_text_input(&crate::TextInputEvent::Insert(typed))
        {
            return;
        }
    }

    /// Hands something to what the pointer is over, topmost first,
    /// until one stops it.
    fn to_hovered<E>(&mut self, pick: impl Fn(&Interactivity) -> SmallVec<[Listener<E>; 2]>, event: &E) {
        let hovered = self.core.hovered.clone();
        for id in hovered {
            let listeners = self.core.nodes.get(id).and_then(|n| n.inter.as_ref()).map(|i| pick(i)).unwrap_or_default();
            self.core.current = Some(id);
            for listener in listeners {
                listener(event, self);
            }
            if !self.core.propagate {
                break;
            }
        }
        self.core.current = None;
    }

    /// Clicks a node as the keyboard does (Space on what has it, Return
    /// on the window's default button). Whether it took the click.
    fn keyboard_click(&mut self, id: NodeId, button: KeyboardButton) -> bool {
        let (clicks, bounds): (SmallVec<[Listener<ClickEvent>; 1]>, _) = match self.core.nodes.get(id) {
            Some(node) if !node.inter.as_ref().is_some_and(|i| i.disabled) => (node.inter.as_ref().map(|i| i.click.clone()).unwrap_or_default(), self.core.window_bounds(id)),
            _ => return false,
        };
        if clicks.is_empty() {
            return false;
        }
        let click = ClickEvent::Keyboard(KeyboardClickEvent { button, bounds });
        self.core.current = Some(id);
        for listener in clicks {
            listener(&click, self);
        }
        self.core.current = None;
        true
    }

    /// Moves the keyboard to the next thing Tab reaches (the one before
    /// with `back`), round and round, in the order things are drawn.
    pub fn focus_next(&mut self, back: bool) {
        let mut stops = Vec::new();
        if let Some(root) = self.core.root {
            self.core.tab_stops(root, &mut stops);
        }
        if stops.is_empty() {
            return;
        }
        let at = self.core.focus.and_then(|f| stops.iter().position(|s| *s == f));
        let next = match (at, back) {
            (Some(i), false) => (i + 1) % stops.len(),
            (Some(i), true) => (i + stops.len() - 1) % stops.len(),
            (None, false) => 0,
            (None, true) => stops.len() - 1,
        };
        self.focus_node(Some(stops[next]));
    }

    /// Hands something to every node that hears the pointer wherever it
    /// is in the window.
    fn to_window_listeners<E>(&mut self, pick: impl Fn(&Interactivity) -> &SmallVec<[Listener<E>; 1]>, event: &E) {
        let listeners: Vec<(NodeId, Listener<E>)> =
            self.core.window_listeners.iter().filter_map(|id| Some((*id, self.core.nodes.get(*id)?.inter.as_ref()?))).flat_map(|(id, i)| pick(i).iter().cloned().map(move |l| (id, l))).collect();
        for (id, listener) in listeners {
            self.core.current = Some(id);
            listener(event, self);
        }
        self.core.current = None;
    }

    /// Hands something up from what has the keyboard through what holds
    /// it.
    fn to_focus_path<E>(&mut self, pick: impl Fn(&Interactivity) -> SmallVec<[Listener<E>; 2]>, event: &E) {
        let path = self.core.focus.map(|f| self.core.path_to(f)).unwrap_or_else(|| self.core.unfocused_path());
        for id in path.iter().rev() {
            let listeners = self.core.nodes.get(*id).and_then(|n| n.inter.as_ref()).map(|i| pick(i)).unwrap_or_default();
            self.core.current = Some(*id);
            for listener in listeners {
                listener(event, self);
            }
            if !self.core.propagate {
                break;
            }
        }
        self.core.current = None;
    }

    /// Gives the keyboard to a node (or to none).
    pub(crate) fn focus_node(&mut self, to: Option<NodeId>) {
        let from = self.core.focus;
        // (Something has the keyboard now: what waited to take it once
        // shown doesn't take it away later.)
        if to.is_some() {
            self.core.pending_focus_handle = None;
        }
        if from == to {
            return;
        }
        self.core.focus = to;
        for (id, focused) in [(from, false), (to, true)] {
            let Some(id) = id else { continue };
            let Some(node) = self.core.nodes.get_mut(id) else { continue };
            node.focused = focused;
            let listeners: SmallVec<[Listener<bool>; 1]> = node.inter.as_ref().map(|i| i.focus_changed.clone()).unwrap_or_default();
            self.core.state_changed(id);
            // (Its listeners run as its own: `invalidate_owner` and
            // `listener_bounds` are this element's, not whatever was
            // being handled when the keyboard moved, or nothing at all:
            // a list given the keyboard as its window opened kept its
            // selection in the colour of one without it.)
            let outer = std::mem::replace(&mut self.core.current, Some(id));
            for listener in listeners {
                listener(&focused, self);
            }
            self.core.current = outer;
        }
        // What holds them and counts the keyboard as its own while it's
        // inside (`focus_within`).
        let old = from.map(|f| self.core.path_to(f)).unwrap_or_default();
        let new = self.core.focus.map(|f| self.core.path_to(f)).unwrap_or_default();
        for id in old.iter().chain(new.iter()) {
            let Some(node) = self.core.nodes.get_mut(*id) else { continue };
            if !node.inter.as_ref().is_some_and(|i| i.focus_within) {
                continue;
            }
            let within = new.contains(id);
            if node.focused != within {
                node.focused = within;
                self.core.state_changed(*id);
            }
        }
        // And what holds them and asked to hear of it: told when the
        // keyboard came into it or left it altogether (a search field's
        // ring, round the text field it holds, went on showing after a
        // click elsewhere took the keyboard).
        let crossed = old.iter().filter(|id| !new.contains(id) && Some(**id) != from).map(|id| (*id, false)).chain(new.iter().filter(|id| !old.contains(id) && Some(**id) != to).map(|id| (*id, true)));
        for (id, within) in crossed.collect::<Vec<_>>() {
            let listeners: SmallVec<[Listener<bool>; 1]> = self.core.nodes.get(id).and_then(|n| n.inter.as_ref()).map(|i| i.focus_changed.clone()).unwrap_or_default();
            if listeners.is_empty() {
                continue;
            }
            let outer = std::mem::replace(&mut self.core.current, Some(id));
            for listener in listeners {
                listener(&within, self);
            }
            self.core.current = outer;
        }
    }

    /// Takes the keyboard from whatever has it.
    pub fn blur(&mut self) {
        self.focus_node(None);
    }

    /// Whether a text of `kind` is what a node shows (for tests and the
    /// curious).
    #[allow(dead_code)]
    pub(crate) fn is_text(&self, id: NodeId) -> bool {
        matches!(self.core.nodes.get(id).map(|n| &n.kind), Some(NodeKind::Text(_)))
    }
}
