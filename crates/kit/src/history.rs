//! Back and Forward through where an app has been (Ben, 2026-11-20:
//! "navigate forward and back should work in passwords app (and every
//! app for that matter)"). A place is whatever the app says it is (a
//! folder; an item, with the search it was found by; a panel), visited
//! as the person goes there; Back and Forward go along the visits
//! without being visits themselves, as a browser's and BeOS's
//! NetPositive's do, and a visit after going back drops what was ahead.
//!
//! An app keeps one `History` of its places, shows Go ▸ Back and Forward
//! (`go_items`: the commands `back` and `forward`, dimmed where there's
//! nowhere to go, their keys every app's in `app_keys`: ⌘[ and ⌘] on a
//! Mac, Alt+← and Alt+→ on Windows), and takes the mouse's back and
//! forward buttons on its window's root
//! (`MouseButton::Navigate(NavigationDirection::Back)`, as Tracker's
//! folder windows do), each going where its command goes.

use crate::Menu;

/// The places an app has shown, and which of them it shows now.
#[derive(Clone, Debug)]
pub struct History<T> {
    places: Vec<T>,
    at: usize,
}

/// The most places kept: the oldest go first.
pub const MOST: usize = 100;

impl<T: Clone + PartialEq> History<T> {
    /// A history starting `here`.
    pub fn new(here: T) -> Self {
        History { places: vec![here], at: 0 }
    }

    /// Where it is now.
    pub fn here(&self) -> &T {
        &self.places[self.at]
    }

    /// The person went to `place` (not by Back or Forward): what was ahead
    /// is let go; going where it already is isn't another visit.
    pub fn visit(&mut self, place: T) {
        if self.places[self.at] == place {
            return;
        }
        self.places.truncate(self.at + 1);
        self.places.push(place);
        if self.places.len() > MOST {
            self.places.remove(0);
        }
        self.at = self.places.len() - 1;
    }

    /// The place shown changed in itself (a search typed in it, a list
    /// scrolled) without the person going anywhere: kept as it is now, so
    /// Back to it comes back to that.
    pub fn update(&mut self, place: T) {
        self.places[self.at] = place;
    }

    pub fn can_back(&self) -> bool {
        self.at > 0
    }

    pub fn can_forward(&self) -> bool {
        self.at + 1 < self.places.len()
    }

    /// The place before, now here; none at the first.
    pub fn back(&mut self) -> Option<T> {
        self.go(-1)
    }

    /// The place after, now here; none at the last.
    pub fn forward(&mut self) -> Option<T> {
        self.go(1)
    }

    /// `by` places along (back when negative), now here.
    pub fn go(&mut self, by: isize) -> Option<T> {
        let to = self.at.checked_add_signed(by).filter(|&i| i < self.places.len())?;
        self.at = to;
        Some(self.places[to].clone())
    }
}

/// Back and Forward, for an app's Go menu: the commands `back` and
/// `forward` (their keys every app's, `app_keys`), dimmed where there's
/// nowhere to go.
pub fn go_items(menu: Menu, can_back: bool, can_forward: bool) -> Menu {
    // (⌘[ and ⌘] declared, as every app's own keys are: the profile's
    // platform gives them their keys there, Alt+← and Alt+→ on Windows.)
    menu.item("Back", "back").key("[").enabled(can_back).item("Forward", "forward").key("]").enabled(can_forward)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Back and Forward go along the visits without being visits; a
    /// visit after going back lets go of what was ahead; the same place
    /// twice is one visit; an update changes where it is, not how many.
    #[test]
    fn back_and_forward_go_along_the_visits() {
        let mut h = History::new("all");
        assert!(!h.can_back() && !h.can_forward());
        h.visit("github");
        h.visit("github");
        h.visit("aws");
        assert_eq!(h.back(), Some("github"));
        assert_eq!(h.back(), Some("all"));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some("github"));
        assert!(h.can_back() && h.can_forward());
        h.visit("mail");
        assert!(!h.can_forward());
        assert_eq!(h.back(), Some("github"));
        h.update("github, searched");
        assert_eq!(h.forward(), Some("mail"));
        assert_eq!(h.back(), Some("github, searched"));
        assert_eq!(h.here(), &"github, searched");
    }

    /// The oldest go first past the most kept.
    #[test]
    fn it_keeps_the_last_hundred() {
        let mut h = History::new(0);
        for i in 1..=150 {
            h.visit(i);
        }
        let mut n = 0;
        while h.back().is_some() {
            n += 1;
        }
        assert_eq!(n, MOST - 1);
        assert_eq!(h.here(), &51);
    }
}
