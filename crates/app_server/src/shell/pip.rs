//! Picture in picture: a video, or a page, popped out of a browser's tab
//! (Meet pops its call out as its tab is left for another). Chrome's are
//! plain toplevels: a video's titled "Picture in picture", a page's with
//! its opener's title and, unlike Chrome's other windows, a largest size.
//! As on macOS, one is small and over every window, in a corner of its
//! display and on each of its workspaces; never in a group, a tile or
//! Focus's zoom (Meet's call joined Chrome's group, zoomed in front of the
//! tab a link had just opened in), never put in another window's
//! remembered place, and opening one leaves the keyboard where it was.

use smithay::{
    utils::{Logical, Point, Size},
    wayland::{
        compositor::with_states,
        shell::xdg::{SurfaceCachedState, XdgToplevelSurfaceData},
    },
};

use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;

use super::WindowElement;
use crate::state::{AnvilState, Backend};

/// Browsers built on Chromium, by their app ids.
const CHROMIUMS: &[&str] = &["google-chrome", "chromium", "brave-browser", "microsoft-edge", "vivaldi", "opera", "thorium"];

/// A video's picture in picture by its title: Chrome's, Firefox's.
pub fn pip_title(title: &str) -> bool {
    matches!(title, "Picture in picture" | "Picture-in-Picture" | "Picture-in-picture")
}

/// Whether a toplevel of `app_id` titled `title`, as big as `max` at the
/// most, with or without a parent, is picture in picture.
pub fn is_pip(app_id: &str, title: &str, max: Size<i32, Logical>, parent: bool) -> bool {
    if pip_title(title) {
        return true;
    }
    // A page in a window of its own (documentPictureInPicture): only those
    // have a largest size (its display's, near enough; the browser's own
    // windows, pop-ups and DevTools none, its dialogs a parent).
    let bounded = |v: i32| v > 0 && v < 1 << 15;
    !parent && CHROMIUMS.iter().any(|c| app_id.starts_with(c)) && (bounded(max.w) || bounded(max.h))
}

impl<B: Backend + 'static> AnvilState<B> {
    /// A window's first commit (before its first configure): whether it is
    /// picture in picture, decided once, from what it said before it. One
    /// that is gives back the keyboard it took as it opened.
    pub fn pip_claim(&mut self, surface: &WlSurface) {
        let Some(w) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return };
        let Some(t) = w.0.toplevel().cloned() else { return };
        if t.is_initial_configure_sent() || w.meta().placed.get() || w.meta().pip.get() {
            return;
        }
        let (app_id, title, max) = with_states(surface, |st| {
            let max = st.cached_state.get::<SurfaceCachedState>().current().max_size;
            let (app_id, title) = st
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok().map(|d| (d.app_id.clone().unwrap_or_default(), d.title.clone().unwrap_or_default())))
                .unwrap_or_default();
            (app_id, title, max)
        });
        if !is_pip(&app_id, &title, max, t.parent().is_some()) {
            return;
        }
        w.meta().pip.set(true);
        *w.meta().grouping.borrow_mut() = super::groups::Grouping::Alone;
        tracing::info!(window = w.id(), app_id, "picture in picture");
        if self.focused_window().as_ref() != Some(&w) {
            return;
        }
        let (before, active) = w.meta().focus_before.borrow().clone();
        if let Some(keyboard) = self.seat.get_keyboard() {
            let before = before.filter(smithay::utils::IsAlive::alive);
            keyboard.set_focus(self, before, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        let active = active.filter(|a| self.space.element_location(a).is_some());
        for e in self.space.elements() {
            smithay::desktop::space::SpaceElement::set_activate(e, active.as_ref() == Some(e));
        }
    }

    /// Its first showing: in the bottom right corner of its display (the
    /// one of its app's window it opened by), as big as it asked. False if
    /// it isn't picture in picture.
    pub fn place_pip(&mut self, surface: &WlSurface) -> bool {
        let Some(w) = self.space.elements().find(|w| w.wl_surface().as_deref() == Some(surface)).cloned() else { return false };
        if !w.meta().pip.get() || w.meta().placed.replace(true) {
            return false;
        }
        w.meta().focus_before.take();
        let (Some(loc), Some(g)) = (self.space.element_location(&w), self.space.element_geometry(&w)) else { return true };
        let Some(area) = self.window_output(&w).or_else(|| self.active_output()).and_then(|o| self.usable_area(&o)) else { return true };
        let room = super::tiling::inset(area, (2 * self.tile_gap()).max(self.settings.tiling.margin.min(400) as i32));
        let x = (room.loc.x + room.size.w - g.size.w).max(area.loc.x);
        let y = (room.loc.y + room.size.h - g.size.h).max(area.loc.y);
        self.space.map_element(w, loc + Point::from((x - g.loc.x, y - g.loc.y)), false);
        self.keep_pips_above();
        true
    }

    /// Picture in picture over every other window, in the order they are.
    pub fn keep_pips_above(&mut self) {
        // (Once a turn: nothing to do unless one is under another window.)
        let mut pip = false;
        if !self.space.elements().any(|w| {
            let p = w.meta().pip.get();
            pip |= p;
            pip && !p
        }) {
            return;
        }
        let pips: Vec<WindowElement> = self.space.elements().filter(|w| w.meta().pip.get()).cloned().collect();
        for w in &pips {
            self.space.raise_element(w, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_picture_in_picture() {
        let none = Size::from((0, 0));
        // Meet's call popped out (documentPictureInPicture), as Chrome 156
        // asks for it on a 1920×1080 display.
        assert!(is_pip("google-chrome", "Meet – abc-defg-hij", Size::from((1512, 840)), false));
        assert!(is_pip("chromium", "Call", Size::from((1512, 840)), false));
        // A video's, by its title.
        assert!(is_pip("google-chrome", "Picture in picture", none, false));
        assert!(is_pip("firefox", "Picture-in-Picture", none, false));
        // Chrome's own windows have no largest size; its dialogs a parent.
        assert!(!is_pip("google-chrome", "Call page - Google Chrome", none, false));
        assert!(!is_pip("google-chrome", "DevTools - example.com", none, false));
        assert!(!is_pip("google-chrome", "Print", Size::from((800, 600)), true));
        // Electron's "no largest size" is i32::MAX; other apps' fixed
        // panels aren't picture in picture.
        assert!(!is_pip("google-chrome", "Call", Size::from((i32::MAX, i32::MAX)), false));
        assert!(!is_pip("olo.deskcalc", "DeskCalc", Size::from((260, 239)), false));
    }
}
