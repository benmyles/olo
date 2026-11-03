//! What the layout review (the kit's `layout_review`) finds in a frame:
//! shared by every kit and the tools that ask for it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Wrong at this scale: fix it.
    Error,
    /// Looks off: look closely, and fix it unless it is meant.
    Review,
}

/// One thing the review found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub level: Level,
    /// What kind: "blurry-icon", "line-weights", "blurry-outline",
    /// "seam", "near-height", "near-level", "near-edge", "off-centre",
    /// "uneven-rows".
    pub kind: String,
    pub text: String,
    /// Where (the first thing it is about), in the window's logical
    /// pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl std::fmt::Display for Finding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Review => "review",
        };
        write!(f, "{level} {}: {} (at {:.0},{:.0} {:.0}×{:.0})", self.kind, self.text, self.x, self.y, self.width, self.height)
    }
}
