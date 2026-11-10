//! What each window shows, for agents: every view says what it is (its
//! role, its name, its value, where it is), and that is kept with the
//! view, so the tree agents read is the view tree itself, there whether
//! or not the window has drawn lately.

use crate::{Bounds, Pixels, SharedString};

/// One thing in a window, as agents read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    /// AccessKit's role: "Button", "CheckBox", "TextInput", "Label", …;
    /// "Clickable" for an area that takes clicks and says nothing more.
    pub role: &'static str,
    pub name: Option<SharedString>,
    pub value: Option<SharedString>,
    /// Where it is in the window, in points.
    pub bounds: Bounds<Pixels>,
    pub enabled: bool,
    pub clickable: bool,
    pub focused: bool,
    pub checked: Option<bool>,
    pub selected: Option<bool>,
    /// A menu item standing for a minimized window.
    pub minimized: bool,
    /// What it was painted in: what shows of it.
    pub clip: Option<Bounds<Pixels>>,
}

impl Node {
    /// Something that takes clicks: a button named `name`.
    pub fn button(name: impl Into<SharedString>, bounds: Bounds<Pixels>) -> Node {
        Node { role: "Button", name: Some(name.into()), bounds, enabled: true, clickable: true, ..Default::default() }
    }

    /// Text drawn in the window.
    pub fn label(text: impl Into<SharedString>, bounds: Bounds<Pixels>) -> Node {
        Node { role: "Label", name: Some(text.into()), bounds, enabled: true, ..Default::default() }
    }
}
