//! The Print panel, as an app shows it in a window of its own (as it
//! does a `FilePanel`): where to (a printer CUPS has, or a PDF), how many
//! copies, which pages, on what paper. The printers are asked for on a
//! worker; until they come the choice is a PDF.

use crate::Paper;

/// The panel on olo-kit (the `kit` feature): a window on a looper of
/// its own.
#[cfg(feature = "kit")]
pub mod kit;

/// The panel's size (points), for its window.
pub const SIZE: (u32, u32) = (380, 196);

/// What was chosen.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    /// The printer (CUPS's name), or None for a PDF.
    pub printer: Option<String>,
    pub copies: u32,
    /// Pages from 1, inclusive; None for all of them.
    pub pages: Option<(u32, u32)>,
    pub paper: Paper,
}
